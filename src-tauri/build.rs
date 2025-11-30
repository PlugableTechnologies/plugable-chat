use std::process::Command;
use std::env;
use std::path::Path;
use std::fs;

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
    
    // Try to build for wasm32-wasi target
    println!("cargo:warning=Attempting to build python-sandbox for WASM...");
    
    let status = Command::new("cargo")
        .args([
            "build",
            "-p", "python-sandbox",
            "--target", "wasm32-wasi",
            "--release",
        ])
        .current_dir(manifest_path)
        .status();
    
    match status {
        Ok(s) if s.success() => {
            // Copy the built WASM to the wasm directory
            let built_wasm = manifest_path
                .join("target")
                .join("wasm32-wasi")
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
            println!("cargo:warning=");
            println!("cargo:warning=To enable WASM sandboxing, install the target:");
            println!("cargo:warning=  rustup target add wasm32-wasi");
            println!("cargo:warning=");
            println!("cargo:warning=code_execution will use RustPython directly (still sandboxed at Python level)");
        }
        Err(e) => {
            println!("cargo:warning=Could not run cargo for WASM build: {}", e);
            println!("cargo:warning=code_execution will use RustPython directly");
        }
    }
}
