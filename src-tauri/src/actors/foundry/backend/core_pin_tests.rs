//! The Foundry Local Core pin: Core 1.2.3 refuses to register GPU providers when its own list
//! repeats a name; 1.2.4 fixes it. These tests fail if the pin drops below 1.2.4 or if a Windows
//! build stops staging it, which would ship the bug again without any compile error.

const PIN: &str = include_str!("../../../../foundry-core-version.txt");
const CI: &str = include_str!("../../../../../.github/workflows/ci.yml");
const RELEASE: &str = include_str!("../../../../../.github/workflows/release.yml");
const FETCH: &str = include_str!("../../../../../scripts/ci/fetch-foundry-core.ps1");

fn parse(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim().split('.').map(|p| p.parse::<u32>().expect("numeric version part"));
    (it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
}

#[test]
fn pinned_core_is_at_least_the_release_that_fixes_the_duplicate_provider_bug() {
    assert!(parse(PIN) >= (1, 2, 4), "pinned Core {} still has the duplicate-provider registration bug", PIN.trim());
}

#[test]
fn windows_builds_stage_the_pinned_core_before_any_cargo_step() {
    for (name, wf) in [("ci.yml", CI), ("release.yml", RELEASE)] {
        let windows = wf.find("runs-on: windows-latest").expect("workflow has a Windows job");
        // The first cargo in the file may belong to a Linux job; compare inside the Windows job.
        let windows_tail = &wf[windows..];
        let stage_in_job = windows_tail.find("fetch-foundry-core.ps1").unwrap_or_else(|| panic!("{name}: Windows job lacks the staging step"));
        let cargo_in_job = windows_tail.find("cargo ").or_else(|| windows_tail.find("tauri.js build")).expect("Windows job builds");
        assert!(stage_in_job < cargo_in_job, "{name}: the Core must be staged before the first build step");
    }
}

#[test]
fn the_fetch_script_reads_the_pin_and_exports_the_override() {
    assert!(FETCH.contains("foundry-core-version.txt"));
    assert!(FETCH.contains("FOUNDRY_NATIVE_OVERRIDE_DIR"));
    assert!(FETCH.contains("microsoft.ai.foundry.local.core.winml"));
}
