//! Plain-language text for catastrophic startup failures (the AI engine cannot start at all).
//!
//! The raw error from the runtime is always included, so a screenshot is enough to diagnose it.
//! Kept free of Tauri types so every branch is unit-testable on any OS.

const RELEASES_URL: &str = "https://github.com/PlugableTechnologies/plugable-chat/releases";
const NVIDIA_DRIVER_URL: &str = "https://www.nvidia.com/Download/index.aspx";
const VC_REDIST_URL: &str = "https://aka.ms/vs/17/release/vc_redist.x64.exe";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupFailure {
    /// The bundled AI runtime could not be loaded and the fallback was used.
    RuntimeLoadFailed { error: String },
    /// The AI service could not be started.
    ServiceStartFailed { error: String },
    /// Startup finished but nothing answered. `runtime_error` is the earlier runtime failure, if any.
    NotConnected { runtime_error: Option<String> },
}

/// True when the error text looks like a missing or unloadable native library.
fn looks_like_missing_library(error: &str) -> bool {
    let e = error.to_lowercase();
    ["not found", "cannot find", "could not find", "no such file", "loadlibrary", "dlopen", "os error 126", "os error 193", "dll"]
        .iter()
        .any(|k| e.contains(k))
}

/// True when the error text shows the runtime could not reach the network (catalog download,
/// proxy, DNS, firewall). Checked before the missing-library test: a stack trace from a network
/// failure can mention a `.dll`, and "reinstall" is the wrong advice for a blocked connection.
fn looks_like_network_failure(error: &str) -> bool {
    let e = error.to_lowercase();
    [
        "httprequestexception",
        "actively refused",
        "os error 10061",
        "os error 10060",
        "os error 11001",
        "catalog request failed",
        "no connection could be made",
        "name or service not known",
        "no such host is known",
        "proxy",
        "network is unreachable",
        "ssl",
        "certificate",
    ]
    .iter()
    .any(|k| e.contains(k))
}

pub fn describe(failure: &StartupFailure, os: Os, log_dir: &str) -> String {
    let (headline, detail) = match failure {
        StartupFailure::RuntimeLoadFailed { error } => (
            "Plugable Chat could not start its AI engine.",
            error.clone(),
        ),
        StartupFailure::ServiceStartFailed { error } => (
            "Plugable Chat could not start its AI service.",
            error.clone(),
        ),
        StartupFailure::NotConnected { runtime_error: Some(error) } => (
            "Plugable Chat could not start its AI engine.",
            error.clone(),
        ),
        StartupFailure::NotConnected { runtime_error: None } => (
            "Plugable Chat started, but the AI engine did not respond.",
            "No error was reported by the engine.".to_string(),
        ),
    };

    let mut steps: Vec<String> = Vec::new();
    steps.push("Close Plugable Chat completely and open it again.".to_string());
    if looks_like_network_failure(&detail) {
        // Network first: reinstalling or updating a driver cannot fix a blocked connection.
        steps.push(
            "Plugable Chat could not reach the internet to download its AI components. Check your internet connection, then open Plugable Chat again.".to_string(),
        );
        steps.push(
            "On a work network, a proxy or firewall may be blocking it. Ask your IT team to allow Microsoft's model catalog and huggingface.co, or set the HTTPS_PROXY setting if your network uses a proxy.".to_string(),
        );
    } else if os == Os::Windows {
        if looks_like_missing_library(&detail) {
            steps.push(format!(
                "The AI engine files may be missing or blocked. Reinstall the latest Plugable Chat from {RELEASES_URL} (you do not need to uninstall first). If antivirus software is installed, check that it did not quarantine files in the Plugable Chat folder."
            ));
            steps.push(format!(
                "Install the Microsoft Visual C++ Redistributable, then reopen Plugable Chat: {VC_REDIST_URL}"
            ));
        } else {
            steps.push(format!(
                "Update your NVIDIA graphics driver, restart the computer, and try again: {NVIDIA_DRIVER_URL}"
            ));
            steps.push(format!(
                "If it still fails, reinstall the latest Plugable Chat from {RELEASES_URL}."
            ));
        }
    } else {
        steps.push(format!(
            "If it still fails, reinstall the latest Plugable Chat from {RELEASES_URL}."
        ));
    }
    steps.push(format!(
        "Still stuck? Send this whole message, plus the files in this folder, to support: {log_dir}"
    ));

    let numbered: Vec<String> = steps
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {}", i + 1, s))
        .collect();
    format!(
        "{headline}\n\nYou do not need to install Foundry Local; Plugable Chat includes what it needs.\n\nWhat to try:\n{}\n\nTechnical detail (for support): {}",
        numbered.join("\n"),
        detail
    )
}

/// `describe` plus the GPU/runtime facts support needs, so one copied message is enough to
/// diagnose a missing driver or Visual C++ runtime without a second round trip.
pub fn describe_with_diagnostics(
    failure: &StartupFailure,
    os: Os,
    log_dir: &str,
    diagnostics: &crate::gpu_diagnostics::GpuDiagnostics,
) -> String {
    format!(
        "{}\n\n{}",
        describe(failure, os, log_dir),
        crate::gpu_diagnostics::format_summary(diagnostics)
    )
}

/// Text for the notice shown when the previous launch never finished starting.
pub fn describe_unclean_previous_launch(os: Os, log_dir: &str, started_at: &str) -> String {
    let mut steps = vec![
        "If you closed Plugable Chat during its first start, this is expected: the first start downloads GPU components and a model and can take several minutes. Leave it open until a model name appears in the header.".to_string(),
        "If the window disappeared on its own, open Plugable Chat again. It usually works the second time.".to_string(),
    ];
    if os == Os::Windows {
        steps.push(format!(
            "If it keeps happening, update your NVIDIA graphics driver, restart the computer, and try again: {NVIDIA_DRIVER_URL}"
        ));
    }
    steps.push(format!(
        "Still stuck? Send this whole message, plus the files in this folder, to support: {log_dir}"
    ));
    let numbered: Vec<String> = steps.iter().enumerate().map(|(i, s)| format!("{}. {}", i + 1, s)).collect();
    format!(
        "Plugable Chat did not finish starting the last time it ran.\n\nWhat to do:\n{}\n\nTechnical detail (for support): previous start began at {}; it never reported that startup finished.",
        numbered.join("\n"),
        started_at
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_blocked_network_gets_network_advice_not_reinstall_or_driver_advice() {
        let f = StartupFailure::NotConnected {
            runtime_error: Some("SDK create failed: RegionFallbackException: Catalog request failed across 7 region(s) ---> System.Net.Http.HttpRequestException: No connection could be made because the target machine actively refused it. (127.0.0.1:9) at Foo.dll".to_string()),
        };
        let text = describe(&f, Os::Windows, "C:\\logs");
        assert!(text.contains("could not reach the internet"), "{text}");
        assert!(text.contains("huggingface.co"), "{text}");
        assert!(!text.contains("Reinstall the latest"), "{text}");
        assert!(!text.contains("NVIDIA graphics driver"), "{text}");
        assert!(text.contains("Technical detail"), "{text}");
    }

    #[test]
    fn unclean_launch_notice_explains_expected_case_and_names_driver_on_windows() {
        let m = super::describe_unclean_previous_launch(super::Os::Windows, "C:\\logs", "t1");
        assert!(m.contains("closed Plugable Chat during its first start"));
        assert!(m.contains(super::NVIDIA_DRIVER_URL) && m.contains("t1"));
        assert!(!super::describe_unclean_previous_launch(super::Os::MacOs, "d", "t").contains(super::NVIDIA_DRIVER_URL));
    }

    use super::*;

    #[test]
    fn windows_missing_library_points_to_reinstall_and_vc_redist() {
        let m = describe(
            &StartupFailure::RuntimeLoadFailed { error: "LoadLibrary failed: os error 126".into() },
            Os::Windows,
            "C:\\logs",
        );
        assert!(m.contains(RELEASES_URL) && m.contains(VC_REDIST_URL));
        assert!(m.contains("os error 126") && m.contains("C:\\logs"));
        assert!(!m.contains("Mac"));
    }

    #[test]
    fn diagnostics_are_appended_after_the_technical_detail() {
        let d = crate::gpu_diagnostics::GpuDiagnostics {
            driver_version: Some("551.61".into()),
            vcpp_installed: Some(false),
            ..Default::default()
        };
        let m = describe_with_diagnostics(
            &StartupFailure::RuntimeLoadFailed { error: "os error 126".into() },
            Os::Windows,
            "C:\\logs",
            &d,
        );
        let detail_at = m.find("Technical detail").unwrap();
        let diag_at = m.find("GPU diagnostics:").unwrap();
        assert!(detail_at < diag_at);
        assert!(m.contains("551.61") && m.contains("NOT FOUND"));
    }

    #[test]
    fn windows_other_error_points_to_gpu_driver() {
        let m = describe(&StartupFailure::ServiceStartFailed { error: "timed out".into() }, Os::Windows, "d");
        assert!(m.contains(NVIDIA_DRIVER_URL) && !m.contains(VC_REDIST_URL));
    }

    #[test]
    fn unreported_failure_says_so_and_never_demands_foundry_install() {
        let m = describe(&StartupFailure::NotConnected { runtime_error: None }, Os::MacOs, "d");
        assert!(m.contains("No error was reported"));
        assert!(m.contains("do not need to install Foundry Local"));
        assert!(!m.contains(NVIDIA_DRIVER_URL));
    }
}
