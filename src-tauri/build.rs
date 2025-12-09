use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let manifest_path = Path::new(&manifest_dir);
    let project_root = manifest_path.parent().unwrap();

    println!("cargo:rerun-if-changed=../src");
    println!("cargo:rerun-if-changed=../index.html");
    println!("cargo:rerun-if-changed=../package.json");
    println!("cargo:rerun-if-changed=../vite.config.ts");
    println!("cargo:rerun-if-changed=../tailwind.config.js");
    println!("cargo:rerun-if-changed=../postcss.config.js");
    println!("cargo:rerun-if-changed=crates/python-sandbox/src");

    let is_windows = cfg!(windows);
    let npm_cmd = if is_windows { "npm.cmd" } else { "npm" };

    // Ensure dependencies are installed
    if !project_root.join("node_modules").exists() {
        println!("cargo:warning=Installing frontend dependencies...");
        let status = Command::new(npm_cmd)
            .arg("install")
            .current_dir(project_root)
            .status()
            .expect("Failed to execute npm install");

        if !status.success() {
            panic!("Frontend dependency installation failed");
        }
    }

    // Try to build python-sandbox WASM module for double-sandbox security
    build_python_sandbox_wasm(manifest_path);

    tauri_build::build()
}

/// Find the rustup executable, checking common locations
fn find_rustup() -> Option<std::path::PathBuf> {
    // First try PATH
    if Command::new("rustup").arg("--version").output().is_ok() {
        return Some("rustup".into());
    }

    // Try common locations
    if let Ok(home) = env::var("HOME") {
        let cargo_bin = Path::new(&home).join(".cargo/bin/rustup");
        if cargo_bin.exists() {
            return Some(cargo_bin);
        }
    }

    if let Ok(home) = env::var("USERPROFILE") {
        let cargo_bin = Path::new(&home).join(".cargo/bin/rustup.exe");
        if cargo_bin.exists() {
            return Some(cargo_bin);
        }
    }

    None
}

/// Check if the wasm32-wasip1 target is installed
/// Note: wasm32-wasi was renamed to wasm32-wasip1 in Rust 1.78+
fn is_wasm_target_installed(rustup: &Path) -> bool {
    let output = Command::new(rustup)
        .args(["target", "list", "--installed"])
        .output();

    match output {
        Ok(out) => {
            let installed = String::from_utf8_lossy(&out.stdout);
            installed.lines().any(|line| {
                let trimmed = line.trim();
                trimmed == "wasm32-wasip1" || trimmed == "wasm32-wasi"
            })
        }
        Err(_) => false,
    }
}

/// Try to install the wasm32-wasip1 target via rustup
fn try_install_wasm_target(rustup: &Path) -> bool {
    println!("cargo:warning=Installing wasm32-wasip1 target via rustup...");

    let status = Command::new(rustup)
        .args(["target", "add", "wasm32-wasip1"])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("cargo:warning=Successfully installed wasm32-wasip1 target");
            true
        }
        Ok(_) => {
            println!("cargo:warning=Failed to install wasm32-wasip1 target");
            false
        }
        Err(e) => {
            println!("cargo:warning=Could not run rustup: {}", e);
            false
        }
    }
}

/// Try to build the python-sandbox crate for WASM
/// This provides the outer WASM sandbox layer
fn build_python_sandbox_wasm(manifest_path: &Path) {
    let wasm_dir = manifest_path.join("wasm");
    let wasm_output = wasm_dir.join("python-sandbox.wasm");

    // Create wasm directory if it doesn't exist
    if !wasm_dir.exists() {
        fs::create_dir_all(&wasm_dir).ok();
    }

    // Check if WASM output already exists and is recent
    if wasm_output.exists() {
        println!("cargo:warning=python-sandbox.wasm found, skipping rebuild");
        return;
    }

    // Find rustup and check/install the wasm32-wasip1 target
    let rustup = match find_rustup() {
        Some(path) => path,
        None => {
            println!("cargo:warning=rustup not found, cannot auto-install wasm32-wasip1 target");
            println!("cargo:warning=To enable WASM sandboxing, install rustup and run:");
            println!("cargo:warning=  rustup target add wasm32-wasip1");
            println!("cargo:warning=");
            println!("cargo:warning=code_execution will use RustPython directly (still sandboxed at Python level)");
            return;
        }
    };

    // Check if wasm32-wasip1 target is installed, try to install if not
    if !is_wasm_target_installed(&rustup) {
        println!("cargo:warning=wasm32-wasip1 target not found, attempting to install...");
        if !try_install_wasm_target(&rustup) {
            println!("cargo:warning=");
            println!("cargo:warning=Could not auto-install wasm32-wasip1 target.");
            println!("cargo:warning=To enable WASM sandboxing, manually run:");
            println!("cargo:warning=  rustup target add wasm32-wasip1");
            println!("cargo:warning=");
            println!("cargo:warning=code_execution will use RustPython directly (still sandboxed at Python level)");
            return;
        }
    }

    // Try to build for wasm32-wasip1 target
    println!("cargo:warning=Building python-sandbox for WASM...");

    let status = Command::new("cargo")
        .args([
            "build",
            "-p",
            "python-sandbox",
            "--target",
            "wasm32-wasip1",
            "--release",
        ])
        .current_dir(manifest_path)
        .status();

    match status {
        Ok(s) if s.success() => {
            // Copy the built WASM to the wasm directory
            let built_wasm = manifest_path
                .join("target")
                .join("wasm32-wasip1")
                .join("release")
                .join("python_sandbox.wasm");

            if built_wasm.exists() {
                if let Err(e) = fs::copy(&built_wasm, &wasm_output) {
                    println!("cargo:warning=Failed to copy WASM: {}", e);
                } else {
                    println!("cargo:warning=Successfully built python-sandbox.wasm");
                }
            } else {
                println!("cargo:warning=WASM build succeeded but output not found");
            }
        }
        Ok(_) => {
            println!("cargo:warning=Failed to build python-sandbox for WASM");
            println!("cargo:warning=code_execution will use RustPython directly (still sandboxed at Python level)");
        }
        Err(e) => {
            println!("cargo:warning=Could not run cargo for WASM build: {}", e);
            println!("cargo:warning=code_execution will use RustPython directly");
        }
    }
}
