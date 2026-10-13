//! The ordered list of models to try, so the app always ends on one that runs.
//!
//! A GPU model is preferred when the GPU is genuinely usable. When it cannot load (or the GPU is
//! not usable at all) the next step is the same model on the CPU, then a small CPU model, so a
//! failure is never the end of the road. Steps that cannot work on this machine (too little RAM
//! or disk, blocklisted after an earlier failure) are listed as skipped with the reason, because
//! "why did it pick the small model?" needs an answer.
//!
//! Pure: no actor or app state, so every ordering is unit-tested.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    Gpu,
    Cpu,
}

impl Device {
    /// `qwen3.5-4b-generic-gpu:4` is a GPU build; ids without a device marker count as CPU.
    pub fn from_model_id(id: &str) -> Device {
        let base = id.to_lowercase();
        let base = base.split(':').next().unwrap_or("").to_string();
        if base.contains("-gpu") || base.contains("-cuda") || base.contains("-trt") || base.contains("-webgpu") || base.contains("-dml") {
            Device::Gpu
        } else {
            Device::Cpu
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub model_id: String,
    pub alias: String,
    pub device: Device,
    /// Download size; 0 when unknown.
    pub size_mb: u64,
    pub cached: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub model_id: String,
    pub device: Device,
    pub cached: bool,
    pub why: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// `(model id, reason)` for candidates that cannot work here.
    pub skipped: Vec<(String, String)>,
}

impl Plan {
    /// The first step after `failed` that needs no download: what to switch to right now.
    pub fn next_cached_after(&self, failed: &str) -> Option<&Step> {
        let pos = self.steps.iter().position(|s| s.model_id.eq_ignore_ascii_case(failed));
        self.steps.iter().skip(pos.map(|p| p + 1).unwrap_or(0)).find(|s| s.cached)
    }

    /// The first step after `failed` of any kind (a download may be needed).
    pub fn next_after(&self, failed: &str) -> Option<&Step> {
        let pos = self.steps.iter().position(|s| s.model_id.eq_ignore_ascii_case(failed));
        self.steps.iter().skip(pos.map(|p| p + 1).unwrap_or(0)).next()
    }
}

pub struct Inputs<'a> {
    pub candidates: &'a [Candidate],
    /// The GPU is usable: an accelerator is registered and GPU models are listed.
    pub gpu_ready: bool,
    /// Dedicated video memory when known. WMI reports at most 4095 MB, so 4095 means "4 GB or
    /// more" and is treated as unknown.
    pub vram_mb: Option<u64>,
    pub ram_mb: Option<u64>,
    pub free_disk_mb: Option<u64>,
    pub is_blocked: &'a dyn Fn(&str) -> bool,
    pub preferred_alias: &'a str,
    pub fallback_alias: &'a str,
}

/// Memory a model needs beyond its file size once loaded (activations, KV cache, the runtime).
const RUN_HEADROOM: f64 = 1.3;
/// Disk needed to download and unpack a model.
const DISK_HEADROOM: f64 = 1.5;

fn matches_alias(c: &Candidate, alias: &str) -> bool {
    let a = alias.to_lowercase();
    c.alias.to_lowercase() == a || c.model_id.to_lowercase().starts_with(&a)
}

pub fn plan_chain(i: &Inputs) -> Plan {
    let mut plan = Plan::default();
    let mut add = |plan: &mut Plan, c: &Candidate, why: &str| {
        if plan.steps.iter().any(|s| s.model_id == c.model_id) || plan.skipped.iter().any(|(m, _)| *m == c.model_id) {
            return;
        }
        if (i.is_blocked)(&c.model_id) {
            plan.skipped.push((c.model_id.clone(), "failed to run here before".to_string()));
            return;
        }
        let size = c.size_mb as f64;
        match c.device {
            Device::Gpu => {
                if !i.gpu_ready {
                    plan.skipped.push((c.model_id.clone(), "GPU acceleration is not available".to_string()));
                    return;
                }
                if let Some(v) = i.vram_mb.filter(|v| *v > 0 && *v < 4095) {
                    if size * RUN_HEADROOM > v as f64 {
                        plan.skipped.push((c.model_id.clone(), format!("needs more video memory than the {v} MB available")));
                        return;
                    }
                }
            }
            Device::Cpu => {
                if let Some(r) = i.ram_mb {
                    if size * RUN_HEADROOM > r as f64 {
                        plan.skipped.push((c.model_id.clone(), format!("needs more memory than the {r} MB installed")));
                        return;
                    }
                }
            }
        }
        if !c.cached {
            if let Some(d) = i.free_disk_mb {
                if size * DISK_HEADROOM > d as f64 {
                    plan.skipped.push((c.model_id.clone(), format!("needs more free disk space than the {d} MB available")));
                    return;
                }
            }
        }
        plan.steps.push(Step { model_id: c.model_id.clone(), device: c.device, cached: c.cached, why: why.to_string() });
    };

    // Within one alias and device, cached builds come first: no wait for a download.
    let mut ordered: Vec<&Candidate> = i.candidates.iter().collect();
    ordered.sort_by_key(|c| (!c.cached, c.size_mb));

    for c in ordered.iter().filter(|c| c.device == Device::Gpu && matches_alias(c, i.preferred_alias)) {
        add(&mut plan, c, "preferred model on the GPU");
    }
    for c in ordered.iter().filter(|c| c.device == Device::Cpu && matches_alias(c, i.preferred_alias)) {
        add(&mut plan, c, "preferred model on the CPU");
    }
    for dev in [Device::Gpu, Device::Cpu] {
        for c in ordered.iter().filter(|c| c.device == dev && matches_alias(c, i.fallback_alias)) {
            add(&mut plan, c, "smaller fallback model");
        }
    }
    // Last resort: anything small enough on the CPU, smallest first.
    let mut rest: Vec<&&Candidate> = ordered.iter().filter(|c| c.device == Device::Cpu).collect();
    rest.sort_by_key(|c| if c.size_mb == 0 { u64::MAX } else { c.size_mb });
    for c in rest {
        add(&mut plan, c, "smallest model that fits");
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, alias: &str, size: u64, cached: bool) -> Candidate {
        Candidate { model_id: id.into(), alias: alias.into(), device: Device::from_model_id(id), size_mb: size, cached }
    }
    fn never(_: &str) -> bool {
        false
    }
    fn catalog() -> Vec<Candidate> {
        vec![
            c("qwen3.5-4b-cuda-gpu:4", "qwen3.5-4b", 2800, false),
            c("qwen3.5-4b-generic-cpu:3", "qwen3.5-4b", 2900, false),
            c("phi-4-mini-instruct-generic-cpu:5", "phi-4-mini-instruct", 2500, false),
            c("qwen2.5-0.5b-instruct-generic-cpu:4", "qwen2.5-0.5b-instruct", 600, false),
        ]
    }
    fn inputs<'a>(cands: &'a [Candidate], gpu_ready: bool, blocked: &'a dyn Fn(&str) -> bool) -> Inputs<'a> {
        Inputs { candidates: cands, gpu_ready, vram_mb: None, ram_mb: Some(16384), free_disk_mb: Some(100_000), is_blocked: blocked, preferred_alias: "qwen3.5-4b", fallback_alias: "phi-4-mini-instruct" }
    }
    fn ids(p: &Plan) -> Vec<&str> {
        p.steps.iter().map(|s| s.model_id.as_str()).collect()
    }

    #[test]
    fn device_comes_from_the_model_id() {
        assert_eq!(Device::from_model_id("qwen3.5-4b-cuda-gpu:4"), Device::Gpu);
        assert_eq!(Device::from_model_id("qwen3.5-4b-generic-cpu:3"), Device::Cpu);
        assert_eq!(Device::from_model_id("phi-4-mini-instruct"), Device::Cpu);
    }

    #[test]
    fn gpu_first_then_the_same_model_on_cpu_then_small_ones() {
        let cat = catalog();
        let p = plan_chain(&inputs(&cat, true, &never));
        assert_eq!(ids(&p), ["qwen3.5-4b-cuda-gpu:4", "qwen3.5-4b-generic-cpu:3", "phi-4-mini-instruct-generic-cpu:5", "qwen2.5-0.5b-instruct-generic-cpu:4"]);
    }

    // R1 (Pat): the GPU is not usable, so the plan starts on the CPU instead of on a GPU model
    // that cannot load.
    #[test]
    fn r1_without_a_usable_gpu_the_plan_starts_on_the_cpu() {
        let cat = catalog();
        let p = plan_chain(&inputs(&cat, false, &never));
        assert_eq!(p.steps[0].model_id, "qwen3.5-4b-generic-cpu:3");
        assert!(p.skipped.iter().any(|(m, why)| m == "qwen3.5-4b-cuda-gpu:4" && why.contains("not available")));
    }

    #[test]
    fn a_blocklisted_gpu_build_falls_through_to_the_cpu_build() {
        let cat = catalog();
        let blocked = |m: &str| m == "qwen3.5-4b-cuda-gpu:4";
        let p = plan_chain(&inputs(&cat, true, &blocked));
        assert_eq!(p.steps[0].model_id, "qwen3.5-4b-generic-cpu:3");
        assert!(p.skipped.iter().any(|(_, why)| why.contains("failed to run here before")));
    }

    #[test]
    fn a_gpu_load_failure_moves_to_the_next_step_not_back_to_the_same_one() {
        let cat = catalog();
        let p = plan_chain(&inputs(&cat, true, &never));
        assert_eq!(p.next_after("qwen3.5-4b-cuda-gpu:4").unwrap().model_id, "qwen3.5-4b-generic-cpu:3");
        assert_eq!(p.next_after("qwen3.5-4b-generic-cpu:3").unwrap().model_id, "phi-4-mini-instruct-generic-cpu:5");
        assert!(p.next_after("qwen2.5-0.5b-instruct-generic-cpu:4").is_none());
    }

    #[test]
    fn next_cached_skips_models_that_would_need_a_download() {
        let mut cat = catalog();
        cat[2].cached = true; // phi is on disk
        let p = plan_chain(&inputs(&cat, true, &never));
        assert_eq!(p.next_cached_after("qwen3.5-4b-cuda-gpu:4").unwrap().model_id, "phi-4-mini-instruct-generic-cpu:5");
    }

    #[test]
    fn a_cached_build_is_preferred_over_one_that_must_download() {
        let mut cat = catalog();
        cat.push(c("qwen3.5-4b-other-cpu:1", "qwen3.5-4b", 3000, true));
        let p = plan_chain(&inputs(&cat, false, &never));
        assert_eq!(p.steps[0].model_id, "qwen3.5-4b-other-cpu:1");
    }

    #[test]
    fn too_little_ram_drops_big_models_but_keeps_a_small_one() {
        let cat = catalog();
        let mut i = inputs(&cat, false, &never);
        i.ram_mb = Some(2048);
        let p = plan_chain(&i);
        assert_eq!(ids(&p), ["qwen2.5-0.5b-instruct-generic-cpu:4"]);
        assert!(p.skipped.iter().any(|(_, why)| why.contains("more memory")));
    }

    #[test]
    fn too_little_disk_drops_downloads_but_not_cached_models() {
        let mut cat = catalog();
        cat[1].cached = true;
        let mut i = inputs(&cat, false, &never);
        i.free_disk_mb = Some(500);
        let p = plan_chain(&i);
        assert_eq!(ids(&p), ["qwen3.5-4b-generic-cpu:3"]);
        assert!(p.skipped.iter().any(|(_, why)| why.contains("free disk space")));
    }

    #[test]
    fn small_video_memory_skips_the_gpu_build_but_a_capped_wmi_reading_does_not() {
        let cat = catalog();
        let mut i = inputs(&cat, true, &never);
        i.vram_mb = Some(2048);
        assert_eq!(plan_chain(&i).steps[0].device, Device::Cpu);
        i.vram_mb = Some(4095); // WMI's 32-bit cap: "at least 4 GB", not a measurement
        assert_eq!(plan_chain(&i).steps[0].device, Device::Gpu);
    }

    #[test]
    fn nothing_fits_gives_an_empty_plan_with_every_reason() {
        let cat = catalog();
        let mut i = inputs(&cat, false, &never);
        i.ram_mb = Some(256);
        let p = plan_chain(&i);
        assert!(p.steps.is_empty());
        assert_eq!(p.skipped.len(), cat.len());
    }
}
