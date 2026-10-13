//! One honest answer to "why am I (not) on the GPU?", built from everything we can observe:
//! what Windows says is installed, what the model engine reports, and what the catalog lists.
//!
//! rc10/rc11 could only say "No GPU models are available". That text was shown for four different
//! situations (no GPU hardware, hardware the engine cannot use, an engine that registered but lists
//! no GPU models, and a catalog that failed to read), so Pat's NVIDIA machine could not be told
//! apart from a laptop with no GPU. Pure functions only; the probes live in `preflight`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
    pub name: String,
    pub vendor: String,
    #[serde(default)]
    pub vram_mb: u64,
    #[serde(default)]
    pub pnp_id: String,
    /// Windows' own status for the device ("OK", "Error", "Degraded", ...).
    #[serde(default)]
    pub status: String,
    /// Non-zero means Device Manager shows a problem for this device.
    #[serde(default)]
    pub problem_code: u32,
}

impl Adapter {
    pub fn is_nvidia(&self) -> bool {
        let v = self.vendor.to_ascii_lowercase();
        let n = self.name.to_ascii_lowercase();
        let p = self.pnp_id.to_ascii_uppercase();
        v.contains("nvidia") || n.contains("nvidia") || n.contains("geforce") || n.contains("quadro") || p.contains("VEN_10DE")
    }

    /// The Microsoft software renderer and remote-desktop display drivers are not GPUs.
    pub fn is_software(&self) -> bool {
        let n = self.name.to_ascii_lowercase();
        n.contains("basic render") || n.contains("basic display") || n.contains("remote display") || n.contains("hyper-v video") || n.contains("virtual display")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterScan {
    pub adapters: Vec<Adapter>,
    /// Names of Thunderbolt controllers/devices Windows lists; context for an external GPU box.
    #[serde(default)]
    pub thunderbolt_devices: Vec<String>,
    /// Set when the scan itself failed, so "found nothing" is never confused with "could not look".
    #[serde(default)]
    pub error: Option<String>,
}

impl AdapterScan {
    pub fn nvidia(&self) -> Vec<&Adapter> {
        self.adapters.iter().filter(|a| a.is_nvidia() && !a.is_software()).collect()
    }
    pub fn real_gpus(&self) -> Vec<&Adapter> {
        self.adapters.iter().filter(|a| !a.is_software()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpObservation {
    pub name: String,
    pub is_registered: bool,
}

/// The model engine's own list of execution providers, or why it could not give one.
pub type EpDiscovery = Result<Vec<EpObservation>, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCounts {
    pub gpu: usize,
    pub cpu: usize,
}

/// Names of providers that run on a GPU or NPU. CPU is deliberately absent.
pub fn is_accelerator_ep(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["cuda", "tensorrt", "nvtensorrtrtx", "webgpu", "directml", "dml", "openvino", "qnn", "vitisai", "migraphx", "rocm"]
        .iter()
        .any(|k| n.contains(k))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuStateKind {
    /// Nothing has been checked yet (start-up still running).
    Checking,
    /// An accelerator is registered and GPU models are listed.
    Ready,
    /// No GPU hardware is installed (software renderer only).
    NoHardware,
    /// Windows lists a GPU the model engine does not use. The Pat case.
    HardwareNotUsed,
    /// The engine registered a GPU provider but its catalog lists no GPU models.
    AcceleratedNoModels,
    /// The model catalog could not be read; says nothing about the GPU.
    CatalogFailed,
    /// The engine could not list providers at all.
    EngineUnavailable,
    /// The adapter scan itself failed and the engine reports no accelerator.
    ScanFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuAssessment {
    pub state: GpuStateKind,
    /// One plain sentence naming the situation.
    pub headline: String,
    /// What each side observed, for the card and the support bundle.
    pub detail: Vec<String>,
    /// What the user can try, most useful first.
    pub hints: Vec<String>,
    /// A retry could change the outcome (as opposed to needing a driver, cable or hardware).
    pub retry_may_help: bool,
}

pub struct AssessInputs<'a> {
    pub scan: Option<&'a AdapterScan>,
    pub eps: Option<&'a EpDiscovery>,
    pub catalog: Option<&'a Result<CatalogCounts, String>>,
    /// `nvidia-smi` driver version when it answered.
    pub driver_version: Option<&'a str>,
    pub vcpp_installed: Option<bool>,
    /// Classified registration failures from `gpu_diagnostics`: `(provider, kind label, message)`.
    pub failures: &'a [(String, String, String)],
}

fn describe_adapters(scan: &AdapterScan) -> String {
    if scan.adapters.is_empty() {
        return "Windows lists no display adapters.".to_string();
    }
    let list: Vec<String> = scan
        .adapters
        .iter()
        .map(|a| {
            let mut s = a.name.clone();
            if a.vram_mb > 0 {
                s.push_str(&format!(", {} MB", a.vram_mb));
            }
            if a.problem_code != 0 {
                s.push_str(&format!(", Device Manager problem code {}", a.problem_code));
            } else if !a.status.is_empty() && !a.status.eq_ignore_ascii_case("ok") {
                s.push_str(&format!(", status {}", a.status));
            }
            s
        })
        .collect();
    format!("Windows lists: {}.", list.join("; "))
}

fn describe_eps(eps: &EpDiscovery) -> String {
    match eps {
        Err(e) => format!("The model engine could not list its providers: {e}"),
        Ok(list) if list.is_empty() => "The model engine reported no execution providers at all.".to_string(),
        Ok(list) => format!(
            "The model engine reports: {}.",
            list.iter().map(|e| format!("{} ({})", e.name, if e.is_registered { "registered" } else { "not registered" })).collect::<Vec<_>>().join(", ")
        ),
    }
}

pub fn assess(i: &AssessInputs) -> GpuAssessment {
    let mut detail = Vec::new();
    if let Some(scan) = i.scan {
        match &scan.error {
            Some(e) => detail.push(format!("Could not list display adapters: {e}")),
            None => detail.push(describe_adapters(scan)),
        }
        if !scan.thunderbolt_devices.is_empty() {
            detail.push(format!("Thunderbolt devices present: {}.", scan.thunderbolt_devices.join("; ")));
        }
    }
    if let Some(eps) = i.eps {
        detail.push(describe_eps(eps));
    }
    match i.driver_version {
        Some(v) => detail.push(format!("NVIDIA driver (nvidia-smi): {v}.")),
        None => detail.push("nvidia-smi did not report a driver version.".to_string()),
    }
    if i.vcpp_installed == Some(false) {
        detail.push("The Visual C++ runtime was not found.".to_string());
    }
    for (ep, kind, msg) in i.failures {
        detail.push(format!("Setting up {ep} failed ({kind}): {msg}"));
    }
    if let Some(Ok(c)) = i.catalog {
        detail.push(format!("Model list: {} GPU, {} CPU.", c.gpu, c.cpu));
    }

    let done = |state, headline: &str, hints: Vec<&str>, retry: bool, detail: Vec<String>| GpuAssessment {
        state,
        headline: headline.to_string(),
        detail,
        hints: hints.into_iter().map(String::from).collect(),
        retry_may_help: retry,
    };

    // A catalog that failed to read says nothing about the GPU; say so before anything else.
    if let Some(Err(e)) = i.catalog {
        let mut d = detail;
        d.push(format!("The model list could not be read: {e}"));
        return done(
            GpuStateKind::CatalogFailed,
            "The model list could not be read, so GPU models cannot be shown.",
            vec!["Choose Retry GPU setup, or restart Plugable Chat.", "If it keeps failing, copy the details for support."],
            true,
            d,
        );
    }

    let eps_ok = i.eps.and_then(|e| e.as_ref().ok());
    let accelerators: Vec<&EpObservation> = eps_ok
        .map(|l| l.iter().filter(|e| e.is_registered && is_accelerator_ep(&e.name)).collect())
        .unwrap_or_default();

    if !accelerators.is_empty() {
        return match i.catalog {
            Some(Ok(c)) if c.gpu == 0 => done(
                GpuStateKind::AcceleratedNoModels,
                "GPU acceleration is registered, but the engine lists no GPU models.",
                vec!["Choose Retry GPU setup to re-read the model list.", "If it persists, copy the details for support."],
                true,
                detail,
            ),
            Some(Ok(_)) => done(GpuStateKind::Ready, "GPU acceleration is ready.", vec![], false, detail),
            // The catalog has not been read yet: do not claim Ready on provider names alone.
            _ => done(GpuStateKind::Checking, "Checking GPU models...", vec![], false, detail),
        };
    }

    let hardware: Vec<&Adapter> = i.scan.map(|s| s.real_gpus()).unwrap_or_default();
    let nvidia: Vec<&Adapter> = i.scan.map(|s| s.nvidia()).unwrap_or_default();
    let thunderbolt = i.scan.map(|s| !s.thunderbolt_devices.is_empty()).unwrap_or(false);

    if let Some(Err(e)) = i.eps {
        let mut d = detail;
        d.push(format!("Engine error: {e}"));
        return done(
            GpuStateKind::EngineUnavailable,
            "The model engine could not report which GPU providers it can use.",
            vec!["Choose Retry GPU setup.", "If it keeps failing, copy the details for support."],
            true,
            d,
        );
    }

    if i.eps.is_none() && i.catalog.is_none() {
        return done(GpuStateKind::Checking, "Checking GPU acceleration...", vec![], false, detail);
    }

    if !hardware.is_empty() {
        let who = nvidia.first().or(hardware.first()).map(|a| a.name.as_str()).unwrap_or("a GPU");
        let mut hints: Vec<&str> = Vec::new();
        let mut retry = true;
        if i.vcpp_installed == Some(false) {
            hints.push("Install the Microsoft Visual C++ Redistributable (x64), then choose Retry GPU setup.");
            retry = false;
        }
        if nvidia.iter().any(|a| a.problem_code != 0) {
            hints.push("Device Manager shows a problem with the NVIDIA device. Reinstall the NVIDIA driver, then restart.");
            retry = false;
        } else if !nvidia.is_empty() && i.driver_version.is_none() {
            hints.push("Windows sees an NVIDIA device, but nvidia-smi did not answer. Install or repair the NVIDIA driver, then restart.");
            retry = false;
        }
        if thunderbolt && !nvidia.is_empty() {
            hints.push("For an external GPU, check the Thunderbolt cable and approve the device if Windows asks, then choose Retry GPU setup.");
        }
        if i.failures.iter().any(|(_, k, _)| k == "network problem") {
            hints.push("Check your internet connection or proxy: GPU components could not be downloaded.");
        }
        hints.push("Choose Retry GPU setup. If it still shows this, copy the details for support.");
        return done(
            GpuStateKind::HardwareNotUsed,
            &format!("Windows sees {who}, but the model engine is not using it, so models run on the CPU."),
            hints,
            retry,
            detail,
        );
    }

    if let Some(scan) = i.scan {
        if scan.error.is_some() {
            return done(
                GpuStateKind::ScanFailed,
                "No GPU acceleration is available, and Windows could not be asked which GPUs are installed.",
                vec!["Choose Retry GPU setup.", "If it keeps failing, copy the details for support."],
                true,
                detail,
            );
        }
    }

    done(
        GpuStateKind::NoHardware,
        "No GPU was found, so models run on the CPU.",
        vec!["A GPU is optional. To use one, connect it and choose Retry GPU setup."],
        false,
        detail,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nv(name: &str) -> Adapter {
        Adapter { name: name.into(), vendor: "NVIDIA".into(), vram_mb: 24576, pnp_id: "PCI\\VEN_10DE&DEV_2684".into(), status: "OK".into(), problem_code: 0 }
    }
    fn igpu() -> Adapter {
        Adapter { name: "Intel(R) UHD Graphics".into(), vendor: "Intel".into(), vram_mb: 128, pnp_id: "PCI\\VEN_8086".into(), status: "OK".into(), problem_code: 0 }
    }
    fn ep(name: &str, reg: bool) -> EpObservation {
        EpObservation { name: name.into(), is_registered: reg }
    }
    fn scan(adapters: Vec<Adapter>) -> AdapterScan {
        AdapterScan { adapters, ..Default::default() }
    }
    fn run(
        scan: Option<&AdapterScan>,
        eps: Option<&EpDiscovery>,
        catalog: Option<&Result<CatalogCounts, String>>,
        driver: Option<&str>,
    ) -> GpuAssessment {
        assess(&AssessInputs { scan, eps, catalog, driver_version: driver, vcpp_installed: Some(true), failures: &[] })
    }

    // R1 (Pat): NVIDIA card, driver installed, engine reports only CPU, so a CPU model loads.
    #[test]
    fn r1_nvidia_present_but_engine_reports_only_cpu() {
        let s = scan(vec![nv("NVIDIA GeForce RTX 4090"), igpu()]);
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true)]);
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 0, cpu: 12 });
        let a = run(Some(&s), Some(&eps), Some(&cat), Some("572.16"));
        assert_eq!(a.state, GpuStateKind::HardwareNotUsed);
        assert!(a.headline.contains("RTX 4090") && a.headline.contains("not using it"), "{}", a.headline);
        let all = a.detail.join("\n");
        assert!(all.contains("CPUExecutionProvider (registered)"), "raw engine answer must be shown: {all}");
        assert!(all.contains("572.16"));
        assert_ne!(a.headline, "No GPU was found, so models run on the CPU.");
    }

    // R2 (Pat): the GPU is an external Thunderbolt box.
    #[test]
    fn r2_external_gpu_gets_a_thunderbolt_hint() {
        let mut s = scan(vec![nv("NVIDIA RTX 4000 Ada")]);
        s.thunderbolt_devices = vec!["Intel(R) Thunderbolt(TM) Controller".into()];
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true)]);
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 0, cpu: 5 });
        let a = run(Some(&s), Some(&eps), Some(&cat), Some("572.16"));
        assert_eq!(a.state, GpuStateKind::HardwareNotUsed);
        assert!(a.hints.iter().any(|h| h.contains("Thunderbolt")), "{:?}", a.hints);
        assert!(a.detail.join("\n").contains("Thunderbolt(TM) Controller"));
    }

    #[test]
    fn nvidia_adapter_without_nvidia_smi_points_at_the_driver() {
        let s = scan(vec![nv("NVIDIA RTX A2000")]);
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true)]);
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 0, cpu: 5 });
        let a = run(Some(&s), Some(&eps), Some(&cat), None);
        assert!(a.hints.iter().any(|h| h.contains("nvidia-smi did not answer")));
        assert!(!a.retry_may_help);
    }

    #[test]
    fn device_manager_problem_code_is_reported() {
        let mut bad = nv("NVIDIA RTX 3060");
        bad.problem_code = 43;
        bad.status = "Error".into();
        let s = scan(vec![bad]);
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true)]);
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 0, cpu: 5 });
        let a = run(Some(&s), Some(&eps), Some(&cat), None);
        assert!(a.detail.join("\n").contains("problem code 43"));
        assert!(a.hints.iter().any(|h| h.contains("Device Manager")));
    }

    #[test]
    fn missing_vcpp_is_called_out() {
        let s = scan(vec![nv("NVIDIA RTX 4090")]);
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true)]);
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 0, cpu: 5 });
        let a = assess(&AssessInputs { scan: Some(&s), eps: Some(&eps), catalog: Some(&cat), driver_version: Some("572.16"), vcpp_installed: Some(false), failures: &[] });
        assert!(a.hints.iter().any(|h| h.contains("Visual C++")));
        assert!(a.detail.join("\n").contains("Visual C++ runtime was not found"));
    }

    #[test]
    fn network_failure_hint_when_components_could_not_download() {
        let s = scan(vec![nv("NVIDIA RTX 4090")]);
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true), ep("CUDAExecutionProvider", false)]);
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 0, cpu: 5 });
        let f = vec![("CUDAExecutionProvider".to_string(), "network problem".to_string(), "HTTP 503".to_string())];
        let a = assess(&AssessInputs { scan: Some(&s), eps: Some(&eps), catalog: Some(&cat), driver_version: Some("572.16"), vcpp_installed: Some(true), failures: &f });
        assert!(a.hints.iter().any(|h| h.contains("internet connection or proxy")));
        assert!(a.detail.join("\n").contains("HTTP 503"));
    }

    // R5: a catalog read failure must never be presented as "no GPU".
    #[test]
    fn r5_catalog_failure_is_not_reported_as_no_gpu() {
        let s = scan(vec![nv("NVIDIA RTX 4090")]);
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true)]);
        let cat: Result<CatalogCounts, String> = Err("connection refused".into());
        let a = run(Some(&s), Some(&eps), Some(&cat), Some("572.16"));
        assert_eq!(a.state, GpuStateKind::CatalogFailed);
        assert!(a.detail.join("\n").contains("connection refused"));
        assert!(!a.headline.to_lowercase().contains("no gpu"));
    }

    #[test]
    fn r5_engine_discovery_failure_is_its_own_state() {
        let s = scan(vec![nv("NVIDIA RTX 4090")]);
        let eps: EpDiscovery = Err("service not started".into());
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 0, cpu: 5 });
        let a = run(Some(&s), Some(&eps), Some(&cat), Some("572.16"));
        assert_eq!(a.state, GpuStateKind::EngineUnavailable);
        assert!(a.detail.join("\n").contains("service not started"));
    }

    #[test]
    fn registered_accelerator_with_no_gpu_models_is_its_own_state() {
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true), ep("CUDAExecutionProvider", true)]);
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 0, cpu: 5 });
        let a = run(Some(&scan(vec![nv("NVIDIA RTX 4090")])), Some(&eps), Some(&cat), Some("572.16"));
        assert_eq!(a.state, GpuStateKind::AcceleratedNoModels);
    }

    #[test]
    fn ready_needs_both_a_registered_accelerator_and_gpu_models() {
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true), ep("CUDAExecutionProvider", true)]);
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 4, cpu: 5 });
        assert_eq!(run(Some(&scan(vec![nv("NVIDIA RTX 4090")])), Some(&eps), Some(&cat), Some("572.16")).state, GpuStateKind::Ready);
        // an unregistered CUDA entry is not an accelerator in use
        let eps2: EpDiscovery = Ok(vec![ep("CUDAExecutionProvider", false)]);
        assert_ne!(run(Some(&scan(vec![nv("NVIDIA RTX 4090")])), Some(&eps2), Some(&cat), Some("572.16")).state, GpuStateKind::Ready);
    }

    #[test]
    fn registered_accelerator_before_the_catalog_is_read_is_still_checking() {
        let eps: EpDiscovery = Ok(vec![ep("CUDAExecutionProvider", true)]);
        assert_eq!(run(None, Some(&eps), None, None).state, GpuStateKind::Checking);
    }

    #[test]
    fn only_a_software_renderer_means_no_hardware() {
        let basic = Adapter { name: "Microsoft Basic Render Driver".into(), vendor: "Microsoft".into(), vram_mb: 0, pnp_id: String::new(), status: "OK".into(), problem_code: 0 };
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true)]);
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 0, cpu: 5 });
        let a = run(Some(&scan(vec![basic])), Some(&eps), Some(&cat), None);
        assert_eq!(a.state, GpuStateKind::NoHardware);
    }

    #[test]
    fn a_failed_adapter_scan_is_not_reported_as_no_hardware() {
        let mut s = scan(vec![]);
        s.error = Some("PowerShell is blocked by policy".into());
        let eps: EpDiscovery = Ok(vec![ep("CPUExecutionProvider", true)]);
        let cat: Result<CatalogCounts, String> = Ok(CatalogCounts { gpu: 0, cpu: 5 });
        let a = run(Some(&s), Some(&eps), Some(&cat), None);
        assert_eq!(a.state, GpuStateKind::ScanFailed);
        assert!(a.detail.join("\n").contains("PowerShell is blocked"));
    }

    #[test]
    fn nothing_observed_yet_is_checking_not_no_gpu() {
        assert_eq!(run(None, None, None, None).state, GpuStateKind::Checking);
    }

    #[test]
    fn vendor_detection() {
        assert!(nv("x").is_nvidia());
        assert!(Adapter { name: "Quadro T1000".into(), vendor: String::new(), vram_mb: 0, pnp_id: String::new(), status: String::new(), problem_code: 0 }.is_nvidia());
        assert!(!igpu().is_nvidia());
        assert!(is_accelerator_ep("CUDAExecutionProvider") && is_accelerator_ep("NvTensorRTRTXExecutionProvider") && is_accelerator_ep("WebGpuExecutionProvider"));
        assert!(!is_accelerator_ep("CPUExecutionProvider"));
    }
}
