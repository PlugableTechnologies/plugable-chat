//! First-run diagnostics and self-service retries.
//!
//! These commands let the UI say why GPU models or the embedding model are unavailable and
//! offer a Retry that works without restarting the app.

use crate::app_state::EmbeddingModelState;
use crate::gpu_diagnostics::{self, GpuDiagnostics};
use std::time::Duration;
use tauri::{AppHandle, State};

/// Why GPU acceleration is or is not available: registered providers, classified failures,
/// NVIDIA driver version and whether the Visual C++ runtime is present.
#[tauri::command]
pub async fn get_gpu_diagnostics() -> Result<GpuDiagnostics, String> {
    tokio::task::spawn_blocking(gpu_diagnostics::snapshot_blocking)
        .await
        .map_err(|e| format!("GPU diagnostics probe failed: {e}"))
}

/// Try GPU provider registration again and wait for the result (backoff included), then return
/// the assessment as it stands. Progress arrives on `ep-registration-progress` meanwhile.
#[tauri::command]
pub async fn retry_gpu_registration(app_handle: AppHandle) -> Result<crate::gpu_assessment::GpuAssessment, String> {
    if crate::actors::foundry::gpu_registration::sdk().is_none() {
        return Err("GPU acceleration setup is not available: the Foundry SDK runtime is not in use.".to_string());
    }
    let outcome = crate::actors::foundry::gpu_registration::retry_now(app_handle).await;
    if !outcome.started {
        return Err("A GPU setup attempt is already running. Wait for it to finish.".to_string());
    }
    Ok(crate::preflight::assessment_now())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineHealthView {
    pub gpu_off: bool,
    pub gpu_off_reason: Option<String>,
    pub unclean_deaths: u32,
    pub last_quarantine: Option<String>,
}

/// Whether the engine's downloaded parts were set aside, or GPU acceleration was switched off,
/// after the app closed unexpectedly.
#[tauri::command]
pub fn get_engine_health() -> EngineHealthView {
    let h = crate::engine_health::health_in(&crate::paths::get_config_dir());
    EngineHealthView {
        gpu_off: crate::engine_health::gpu_off(),
        gpu_off_reason: h.gpu_off_reason,
        unclean_deaths: h.unclean_deaths,
        last_quarantine: h.last_quarantine,
    }
}

/// Settings > Models > Repair: set the GPU components aside so they download again, clear crash
/// history and engine-blamed model blocks, then run GPU setup. No reinstall or reboot needed.
#[tauri::command]
pub async fn repair_engine(
    app_handle: AppHandle,
    settings_state: State<'_, crate::app_state::SettingsState>,
) -> Result<String, String> {
    let line = {
        let mut guard = settings_state.settings.write().await;
        let line = crate::engine_health::repair(&mut guard);
        crate::settings::save_settings(&guard).await?;
        line
    };
    crate::app_log::log("repair", &format!("Repair requested: {line}"));
    crate::actors::foundry::gpu_registration::spawn_retry(app_handle);
    Ok(line)
}

/// "Try GPU again" after GPU acceleration was switched off: turn it on and run GPU setup.
#[tauri::command]
pub async fn try_gpu_again(app_handle: AppHandle) -> Result<(), String> {
    let dir = crate::paths::get_config_dir();
    let mut h = crate::engine_health::health_in(&dir);
    h.gpu_off = false;
    h.gpu_off_reason = None;
    h.unclean_deaths = 0;
    crate::engine_health::save_health_in(&dir, &h);
    crate::engine_health::set_gpu_off(false);
    crate::actors::foundry::gpu_registration::spawn_retry(app_handle);
    Ok(())
}

/// Why the GPU is or is not in use, from Windows' view and the model engine's view together.
/// `refresh` asks Windows again; the answer is never reused from a failed earlier look.
#[tauri::command]
pub async fn get_gpu_assessment(refresh: Option<bool>) -> Result<crate::gpu_assessment::GpuAssessment, String> {
    tokio::task::spawn_blocking(move || {
        if refresh.unwrap_or(false) {
            crate::preflight::refresh_blocking()
        } else {
            crate::preflight::assessment_now()
        }
    })
    .await
    .map_err(|e| format!("GPU assessment failed: {e}"))
}

/// Hosts the app must reach, found in the code and scripts: the search model (Hugging Face and its
/// file CDN), provider packages (NuGet), the database toolbox, and the Visual C++ runtime link. The
/// model engine's own catalog host lives inside the Foundry SDK and is not named in this repo.
pub const REACHABILITY_HOSTS: [&str; 5] = [
    "huggingface.co",
    "cdn-lfs.huggingface.co",
    "api.nuget.org",
    "storage.googleapis.com",
    "aka.ms",
];

/// Result of trying one host: either an HTTP status (any answer proves the path works) or why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostOutcome {
    Reachable { status: u16, millis: u128 },
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostCheck {
    pub host: String,
    pub outcome: HostOutcome,
}

/// Plain-language reason for a failed request; names a reset as a likely firewall/antivirus/proxy.
pub fn describe_host_failure(raw: &str) -> String {
    use crate::actors::foundry::embedding_init::is_connection_reset;
    let l = raw.to_ascii_lowercase();
    let cause = if is_connection_reset(raw) {
        "connection reset (a firewall, antivirus or proxy may be blocking it)"
    } else if l.contains("dns") || l.contains("failed to lookup") || l.contains("no such host") || l.contains("os error 11001") {
        "name did not resolve"
    } else if l.contains("certificate") || l.contains("unknownissuer") {
        "certificate problem"
    } else if l.contains("timed out") || l.contains("timeout") || l.contains("os error 10060") {
        "timed out"
    } else {
        "could not connect"
    };
    format!("{cause}: {raw}")
}

/// One line per host for the support bundle.
pub fn format_host_checks(checks: &[HostCheck]) -> String {
    let mut out = String::from("Network reachability (HTTPS, this machine, this moment):");
    for c in checks {
        match &c.outcome {
            HostOutcome::Reachable { status, millis } => out.push_str(&format!("\n  {}: reachable (HTTP {status}, {millis} ms)", c.host)),
            HostOutcome::Failed { reason } => out.push_str(&format!("\n  {}: FAILED - {reason}", c.host)),
        }
    }
    out
}

async fn check_one_host(client: reqwest::Client, host: String) -> HostCheck {
    let started = std::time::Instant::now();
    let outcome = match client.head(format!("https://{host}/")).send().await {
        Ok(resp) => HostOutcome::Reachable { status: resp.status().as_u16(), millis: started.elapsed().as_millis() },
        Err(e) => {
            let mut raw = e.to_string();
            let mut src = std::error::Error::source(&e);
            while let Some(inner) = src {
                raw.push_str(&format!(": {inner}"));
                src = inner.source();
            }
            HostOutcome::Failed { reason: describe_host_failure(&raw) }
        }
    };
    HostCheck { host, outcome }
}

/// Try every host in `REACHABILITY_HOSTS` at the same time, each bounded to 6 seconds. Uses the same
/// proxy and certificate settings the app's own downloads use.
pub async fn check_host_reachability() -> Vec<HostCheck> {
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(6)).build() {
        Ok(c) => c,
        Err(e) => {
            return vec![HostCheck { host: "(http client)".to_string(), outcome: HostOutcome::Failed { reason: e.to_string() } }];
        }
    };
    let tasks: Vec<_> = REACHABILITY_HOSTS
        .iter()
        .map(|h| tokio::spawn(check_one_host(client.clone(), h.to_string())))
        .collect();
    let mut out = Vec::new();
    for (task, host) in tasks.into_iter().zip(REACHABILITY_HOSTS) {
        out.push(task.await.unwrap_or_else(|e| HostCheck { host: host.to_string(), outcome: HostOutcome::Failed { reason: e.to_string() } }));
    }
    out
}

/// Everything support needs in one block of text: versions, machine facts, the GPU view from both
/// sides, and the last lines of the app log. Written to the logs folder as well.
#[tauri::command]
pub async fn get_support_bundle(extra: Option<String>) -> Result<String, String> {
    // A blocked host (the rc16 failure) is invisible in the log tail; ask each one directly.
    let reachability = format_host_checks(&check_host_reachability().await);
    tokio::task::spawn_blocking(move || {
        let report = crate::preflight::build_report_blocking();
        let mut text = crate::preflight::format_report(&report);
        if let Some(extra) = extra.filter(|e| !e.trim().is_empty()) {
            text.push_str("\n\nWhat the app showed:\n");
            text.push_str(extra.trim());
        }
        text.push_str("\n\n");
        text.push_str(&reachability);
        let tail = crate::app_log::tail(60);
        if !tail.is_empty() {
            text.push_str("\n\nRecent log:\n");
            text.push_str(&tail.join("\n"));
        }
        // The engine's reason for skipping a provider is only in its own log.
        let engine_dir = crate::app_log::engine_log_dir();
        match crate::app_log::engine_log_excerpt_in(&engine_dir, 40) {
            Some((path, lines)) => {
                text.push_str(&format!("\n\nModel engine log ({}):\n", path.display()));
                text.push_str(if lines.is_empty() { "(no provider or error lines)" } else { "" });
                text.push_str(&lines.join("\n"));
            }
            None => text.push_str(&format!("\n\nModel engine log: nothing written in {}", engine_dir.display())),
        }
        text
    })
    .await
    .map_err(|e| format!("could not collect support details: {e}"))
}

/// Run the embedding-model start-up again (download included) after it failed.
#[tauri::command]
pub async fn retry_embedding_init(
    app_handle: AppHandle,
    embedding_state: State<'_, EmbeddingModelState>,
) -> Result<(), String> {
    crate::actors::foundry::embedding_init::init_cpu_embedding_model(app_handle, embedding_state.cpu_model.clone()).await
}

#[cfg(test)]
mod reachability_tests {
    use super::*;

    #[test]
    fn the_hosts_checked_include_the_search_model_and_its_cdn() {
        assert!(REACHABILITY_HOSTS.contains(&"huggingface.co") && REACHABILITY_HOSTS.contains(&"cdn-lfs.huggingface.co"));
    }

    #[test]
    fn a_reset_is_reported_as_a_likely_firewall_block() {
        let r = describe_host_failure("error sending request: tls connection init failed: forcibly closed by the remote host (os error 10054)");
        assert!(r.starts_with("connection reset (a firewall, antivirus or proxy"), "{r}");
        assert!(describe_host_failure("dns error: failed to lookup address").starts_with("name did not resolve"));
        assert!(describe_host_failure("invalid peer certificate: UnknownIssuer").starts_with("certificate problem"));
        assert!(describe_host_failure("operation timed out").starts_with("timed out"));
    }

    #[test]
    fn the_bundle_section_has_one_line_per_host() {
        let text = format_host_checks(&[
            HostCheck { host: "huggingface.co".into(), outcome: HostOutcome::Reachable { status: 200, millis: 41 } },
            HostCheck { host: "api.nuget.org".into(), outcome: HostOutcome::Failed { reason: "timed out: x".into() } },
        ]);
        assert!(text.contains("huggingface.co: reachable (HTTP 200, 41 ms)"), "{text}");
        assert!(text.contains("api.nuget.org: FAILED - timed out: x"), "{text}");
        assert_eq!(text.lines().count(), 3);
    }
}
