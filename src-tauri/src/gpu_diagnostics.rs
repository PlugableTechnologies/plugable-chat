//! Why GPU acceleration is (not) available, in a form the UI and the support bundle can show.
//!
//! The Foundry SDK reports a failed execution provider (EP) as a name plus free text. This
//! module classifies that text, remembers the outcome of every registration attempt, and
//! probes the host (NVIDIA driver, Visual C++ runtime) so "no GPU models" has a stated cause.
//! Everything here is free of Tauri types and, apart from `probe_environment`, pure.

use crate::process_utils::HideConsoleWindow;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpFailureKind {
    /// Download or catalog request failed (offline, proxy, TLS, HTTP error).
    Network,
    /// The GPU driver is missing or older than the provider needs.
    Driver,
    /// A native library (including the Visual C++ runtime) is missing or cannot be loaded.
    MissingDll,
    /// The user stopped the download.
    Cancelled,
    /// The hardware or OS cannot run this provider.
    Unsupported,
    Unknown,
}

impl EpFailureKind {
    /// Worth trying again later without the user changing anything.
    pub fn is_retryable(self) -> bool {
        matches!(self, EpFailureKind::Network | EpFailureKind::Unknown)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpFailure {
    pub ep: String,
    pub kind: EpFailureKind,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuDiagnostics {
    pub registered: Vec<String>,
    pub failed: Vec<EpFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcpp_installed: Option<bool>,
}

/// Classify the text the SDK or the OS gave for a failed provider registration.
///
/// Order matters: a missing library is checked before the driver because a CUDA runtime DLL
/// that cannot be found names "cuda" too, and a driver problem is checked before the network
/// because driver messages often say "version" and "insufficient".
pub fn classify_ep_error(message: &str) -> EpFailureKind {
    let m = message.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| m.contains(n));

    if has(&["cancel", "aborted by user", "operation was aborted"]) {
        return EpFailureKind::Cancelled;
    }
    if has(&[
        "os error 126",
        "os error 193",
        "error 126",
        "error 193",
        "0x8007007e",
        "0x800700c1",
        "vcruntime",
        "msvcp",
        "vcomp",
        "loadlibrary",
        "dlopen",
        "cannot open shared object",
        ".dll",
        " dll",
        "specified module could not be found",
        "not a valid win32 application",
    ]) {
        return EpFailureKind::MissingDll;
    }
    if has(&[
        "driver",
        "cuda failure 35",
        "nvcuda",
        "nvml",
        "too old",
        "insufficient",
    ]) {
        return EpFailureKind::Driver;
    }
    if has(&[
        "no cuda-capable",
        "no cuda capable",
        "unsupported",
        "not supported",
        "no compatible",
        "no nvidia",
        "no gpu",
        "not available on this",
        "no suitable device",
    ]) {
        return EpFailureKind::Unsupported;
    }
    if has(&[
        "http",
        "timed out",
        "timeout",
        "dns",
        "connection",
        "connect error",
        "tls",
        "ssl",
        "certificate",
        "proxy",
        "unreachable",
        "network",
        "error sending request",
        "name resolution",
        "403",
        "404",
        "429",
        "500",
        "502",
        "503",
        "504",
    ]) {
        return EpFailureKind::Network;
    }
    EpFailureKind::Unknown
}

pub fn make_failure(ep: &str, message: &str) -> EpFailure {
    EpFailure {
        ep: ep.to_string(),
        kind: classify_ep_error(message),
        message: message.to_string(),
    }
}

/// Fold one registration attempt into the remembered state. An EP that registered now is no
/// longer failed; an EP that failed again takes the newest message.
pub fn merge_attempt(
    previous: &GpuDiagnostics,
    registered_now: &[String],
    failures_now: &[EpFailure],
) -> GpuDiagnostics {
    let mut registered = previous.registered.clone();
    for ep in registered_now {
        if !registered.contains(ep) {
            registered.push(ep.clone());
        }
    }
    let mut failed: Vec<EpFailure> = previous
        .failed
        .iter()
        .filter(|f| !registered.contains(&f.ep) && !failures_now.iter().any(|n| n.ep == f.ep))
        .cloned()
        .collect();
    failed.extend(failures_now.iter().filter(|f| !registered.contains(&f.ep)).cloned());
    GpuDiagnostics {
        registered,
        failed,
        driver_version: previous.driver_version.clone(),
        vcpp_installed: previous.vcpp_installed,
    }
}

fn state() -> &'static Mutex<GpuDiagnostics> {
    static STATE: std::sync::OnceLock<Mutex<GpuDiagnostics>> = std::sync::OnceLock::new();
    STATE.get_or_init(Default::default)
}

fn probe_cache() -> &'static Mutex<Option<(Option<String>, Option<bool>)>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<(Option<String>, Option<bool>)>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Remember the outcome of a registration attempt.
pub fn record_attempt(registered_now: &[String], failures_now: &[EpFailure]) {
    if let Ok(mut s) = state().lock() {
        *s = merge_attempt(&s, registered_now, failures_now);
    }
}

/// Forget cached host probes (a retry may follow a driver or runtime install).
pub fn invalidate_probe() {
    if let Ok(mut c) = probe_cache().lock() {
        *c = None;
    }
}

/// Current diagnostics without probing the host.
pub fn snapshot_without_probe() -> GpuDiagnostics {
    let mut d = state().lock().map(|s| s.clone()).unwrap_or_default();
    if let Some((driver, vcpp)) = probe_cache().lock().ok().and_then(|c| c.clone()) {
        d.driver_version = driver;
        d.vcpp_installed = vcpp;
    }
    d
}

/// Current diagnostics including a host probe. Blocking (spawns `nvidia-smi` / `reg`).
pub fn snapshot_blocking() -> GpuDiagnostics {
    let cached = probe_cache().lock().ok().and_then(|c| c.clone());
    if cached.is_none() {
        let probed = probe_environment();
        if let Ok(mut c) = probe_cache().lock() {
            *c = Some(probed);
        }
    }
    snapshot_without_probe()
}

/// First non-empty line of `nvidia-smi --query-gpu=driver_version --format=csv,noheader`.
pub fn parse_nvidia_smi_driver(output: &str) -> Option<String> {
    output
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .filter(|l| l.chars().next().map_or(false, |c| c.is_ascii_digit()))
        .map(str::to_string)
}

/// True when `reg query ... /v Installed` output says the value is 1.
pub fn parse_reg_installed(output: &str) -> bool {
    output.lines().any(|line| {
        let mut parts = line.split_whitespace();
        parts.next().map_or(false, |n| n.eq_ignore_ascii_case("Installed"))
            && parts.last().map_or(false, |v| v.eq_ignore_ascii_case("0x1"))
    })
}

fn probe_driver_version() -> Option<String> {
    let out = std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=driver_version", "--format=csv,noheader"])
        .hide_console_window()
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_nvidia_smi_driver(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(windows)]
fn probe_vcpp_installed() -> Option<bool> {
    let registry = std::process::Command::new("reg")
        .args([
            "query",
            r"HKLM\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64",
            "/v",
            "Installed",
        ])
        .hide_console_window()
        .output()
        .ok()
        .map(|o| parse_reg_installed(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false);
    // The runtime DLL is the thing the app needs; the registry key can be missing on images
    // that ship the DLL without running the redistributable installer.
    let dll = std::env::var_os("SystemRoot")
        .map(|root| std::path::Path::new(&root).join("System32").join("vcruntime140.dll").exists())
        .unwrap_or(false);
    Some(registry || dll)
}

#[cfg(not(windows))]
fn probe_vcpp_installed() -> Option<bool> {
    None
}

/// `(nvidia driver version, VC++ runtime present)`; `None` where it cannot be determined.
pub fn probe_environment() -> (Option<String>, Option<bool>) {
    (probe_driver_version(), probe_vcpp_installed())
}

/// Plain-text block for the "copy details for support" bundle.
pub fn format_summary(d: &GpuDiagnostics) -> String {
    let mut lines = vec!["GPU diagnostics:".to_string()];
    lines.push(format!(
        "  registered providers: {}",
        if d.registered.is_empty() { "none".to_string() } else { d.registered.join(", ") }
    ));
    if d.failed.is_empty() {
        lines.push("  failed providers: none".to_string());
    } else {
        for f in &d.failed {
            lines.push(format!("  failed: {} [{:?}] {}", f.ep, f.kind, f.message));
        }
    }
    lines.push(format!(
        "  NVIDIA driver: {}",
        d.driver_version.as_deref().unwrap_or("not detected")
    ));
    lines.push(format!(
        "  Visual C++ runtime: {}",
        match d.vcpp_installed {
            Some(true) => "installed",
            Some(false) => "NOT FOUND",
            None => "not checked on this OS",
        }
    ));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use EpFailureKind::*;

    #[test]
    fn windows_library_load_errors_are_missing_dll() {
        for msg in [
            "LoadLibrary failed with error 126: The specified module could not be found.",
            "Failed to load onnxruntime_providers_cuda.dll (os error 126)",
            "LoadLibraryExW failed: %1 is not a valid Win32 application (os error 193)",
            "The code execution cannot proceed because VCRUNTIME140_1.dll was not found",
            "vcruntime140 missing",
            "error loading MSVCP140.dll",
            "dlopen(libcudart.so.12) failed",
        ] {
            assert_eq!(classify_ep_error(msg), MissingDll, "{msg}");
        }
    }

    #[test]
    fn cuda_runtime_dll_is_missing_dll_not_driver() {
        assert_eq!(classify_ep_error("cudart64_12.dll not found"), MissingDll);
    }

    #[test]
    fn driver_messages_are_driver() {
        for msg in [
            "CUDA driver version is insufficient for CUDA runtime version",
            "CUDA failure 35: CUDA driver version is insufficient",
            "NVIDIA driver too old: need 550 or newer",
            "nvml initialization failed",
        ] {
            assert_eq!(classify_ep_error(msg), Driver, "{msg}");
        }
    }

    #[test]
    fn missing_hardware_is_unsupported() {
        for msg in [
            "no CUDA-capable device is detected",
            "CUDAExecutionProvider is not supported on this machine",
            "No compatible GPU found",
        ] {
            assert_eq!(classify_ep_error(msg), Unsupported, "{msg}");
        }
    }

    #[test]
    fn http_and_transport_errors_are_network() {
        for msg in [
            "HTTP 503 Service Unavailable",
            "download failed: HTTP status 404",
            "error sending request for url (https://huggingface.co/x): connection timed out",
            "tls handshake failure: invalid peer certificate",
            "dns error: failed to lookup address",
            "proxy authentication required",
        ] {
            assert_eq!(classify_ep_error(msg), Network, "{msg}");
        }
    }

    #[test]
    fn cancellation_wins_over_everything() {
        assert_eq!(classify_ep_error("Download cancelled by user (http stream closed)"), Cancelled);
    }

    #[test]
    fn unrecognised_text_is_unknown_and_empty_is_unknown() {
        assert_eq!(classify_ep_error("something odd happened"), Unknown);
        assert_eq!(classify_ep_error(""), Unknown);
    }

    #[test]
    fn only_network_and_unknown_are_retryable() {
        assert!(Network.is_retryable() && Unknown.is_retryable());
        for k in [Driver, MissingDll, Cancelled, Unsupported] {
            assert!(!k.is_retryable());
        }
    }

    #[test]
    fn kind_serialises_as_snake_case() {
        assert_eq!(serde_json::to_string(&MissingDll).unwrap(), "\"missing_dll\"");
        let f = make_failure("CUDAExecutionProvider", "os error 126");
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["ep"], "CUDAExecutionProvider");
        assert_eq!(v["kind"], "missing_dll");
    }

    #[test]
    fn optional_fields_are_omitted_when_unknown() {
        let v = serde_json::to_value(GpuDiagnostics::default()).unwrap();
        assert!(v.get("driver_version").is_none() && v.get("vcpp_installed").is_none());
        assert!(v["registered"].is_array() && v["failed"].is_array());
    }

    #[test]
    fn merge_clears_a_failure_once_the_provider_registers() {
        let first = merge_attempt(
            &GpuDiagnostics::default(),
            &["CPUExecutionProvider".into()],
            &[make_failure("CUDAExecutionProvider", "HTTP 503")],
        );
        assert_eq!(first.failed.len(), 1);
        let second = merge_attempt(&first, &["CUDAExecutionProvider".into()], &[]);
        assert!(second.failed.is_empty());
        assert_eq!(second.registered, ["CPUExecutionProvider", "CUDAExecutionProvider"]);
    }

    #[test]
    fn merge_replaces_an_old_message_for_the_same_provider() {
        let first = merge_attempt(&GpuDiagnostics::default(), &[], &[make_failure("CUDA", "HTTP 503")]);
        let second = merge_attempt(&first, &[], &[make_failure("CUDA", "os error 126")]);
        assert_eq!(second.failed.len(), 1);
        assert_eq!(second.failed[0].kind, MissingDll);
    }

    #[test]
    fn nvidia_smi_output_parsing() {
        assert_eq!(parse_nvidia_smi_driver("551.61\n"), Some("551.61".into()));
        assert_eq!(parse_nvidia_smi_driver("\n  535.104.05  \n551.61\n"), Some("535.104.05".into()));
        assert_eq!(parse_nvidia_smi_driver("NVIDIA-SMI has failed because it couldn't communicate"), None);
        assert_eq!(parse_nvidia_smi_driver(""), None);
    }

    #[test]
    fn reg_query_output_parsing() {
        let yes = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\x64\r\n    Installed    REG_DWORD    0x1\r\n";
        let no = "    Installed    REG_DWORD    0x0\r\n";
        assert!(parse_reg_installed(yes));
        assert!(!parse_reg_installed(no));
        assert!(!parse_reg_installed("ERROR: The system was unable to find the specified registry key or value."));
    }

    #[test]
    fn summary_names_every_cause_for_support() {
        let d = GpuDiagnostics {
            registered: vec!["CPUExecutionProvider".into()],
            failed: vec![make_failure("CUDAExecutionProvider", "os error 126")],
            driver_version: Some("551.61".into()),
            vcpp_installed: Some(false),
        };
        let s = format_summary(&d);
        assert!(s.contains("CUDAExecutionProvider") && s.contains("MissingDll") && s.contains("os error 126"));
        assert!(s.contains("551.61") && s.contains("NOT FOUND"));
    }
}
