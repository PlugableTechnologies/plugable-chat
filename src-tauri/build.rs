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

    // Get git commit count and hash for versioning
    set_git_version_info(project_root);

    // Clean up macOS AppleDouble files (._*) from capabilities directory before tauri_build scans it.
    // These files are created when extracting a zip on Windows that was created on macOS.
    clean_apple_double_files(manifest_path.join("capabilities").as_path());

    // ==========================================================================
    // GPU EMBEDDING DISABLED - The following ONNX-related build steps are
    // commented out. To re-enable, uncomment these and the corresponding
    // dependency blocks in Cargo.toml.
    // ==========================================================================
    
    // // Link clang runtime on macOS for ONNX Runtime CoreML support
    // #[cfg(target_os = "macos")]
    // link_macos_clang_runtime();

    // // Copy ONNX Runtime DLLs to binaries directory on Windows for bundling
    // #[cfg(target_os = "windows")]
    // copy_onnx_runtime_dlls(manifest_path);

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

/// Remove macOS AppleDouble files (._*) from a directory.
/// These files are created when copying files to non-HFS volumes (e.g., when extracting a zip).
/// They contain invalid UTF-8 and cause tauri_build to fail when scanning the capabilities directory.
fn clean_apple_double_files(dir: &Path) {
    if !dir.exists() {
        return;
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();
            if file_name_str.starts_with("._") {
                let path = entry.path();
                if let Err(e) = fs::remove_file(&path) {
                    println!(
                        "cargo:warning=Failed to remove AppleDouble file {:?}: {}",
                        path, e
                    );
                } else {
                    println!(
                        "cargo:warning=Removed macOS AppleDouble file: {:?}",
                        file_name_str
                    );
                }
            }
        }
    }
}

/// Get git commit count and short hash, export as environment variables
fn set_git_version_info(project_root: &Path) -> u32 {
    // Get git commit count
    let commit_count = Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(project_root)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
            } else {
                None
            }
        })
        .unwrap_or(0);

    // Get short git hash
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(project_root)
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok().map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());

    // Export as environment variables for use in Rust code
    println!("cargo:rustc-env=PLUGABLE_CHAT_GIT_COUNT={}", commit_count);
    println!("cargo:rustc-env=PLUGABLE_CHAT_GIT_HASH={}", git_hash);

    commit_count
}

/// Link the clang runtime library on macOS for ONNX Runtime CoreML support.
///
/// The pre-built ONNX Runtime binaries with CoreML use `@available()` checks which
/// require the `___isPlatformVersionAtLeast` symbol from Apple's clang runtime.
/// Rust's default linker invocation doesn't include this library, so we need to
/// explicitly add it.
///
/// This function dynamically finds the correct clang version directory to work
/// across different Xcode versions (clang/16, clang/17, clang/18, etc.).
/// 
/// NOTE: GPU EMBEDDING DISABLED - This function is currently unused. To re-enable,
/// uncomment the call site in main() and the ort dependencies in Cargo.toml.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
fn link_macos_clang_runtime() {
    // Find the Xcode Developer directory using xcode-select
    let developer_output = Command::new("xcode-select").args(["-p"]).output();

    let toolchain_base = match developer_output {
        Ok(output) if output.status.success() => {
            // xcode-select -p gives us: /Applications/Xcode.app/Contents/Developer
            let developer_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            Path::new(&developer_path)
                .join("Toolchains")
                .join("XcodeDefault.xctoolchain")
                .join("usr")
                .join("lib")
                .join("clang")
        }
        _ => {
            println!("cargo:warning=xcode-select not available, cannot find clang runtime");
            return;
        }
    };

    // Find the clang version directory (e.g., clang/17, clang/18)
    let clang_version_dir = match fs::read_dir(&toolchain_base) {
        Ok(entries) => {
            let mut versions: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    // Parse version number (could be "17", "17.0.0", etc.)
                    name.split('.').next()?.parse::<u32>().ok().map(|v| (v, e.path()))
                })
                .collect();

            // Sort by version number descending, use the latest
            versions.sort_by(|a, b| b.0.cmp(&a.0));
            versions.into_iter().next().map(|(_, path)| path)
        }
        Err(e) => {
            println!(
                "cargo:warning=Could not read clang directory {:?}: {}",
                toolchain_base, e
            );
            return;
        }
    };

    let clang_dir = match clang_version_dir {
        Some(dir) => dir,
        None => {
            println!(
                "cargo:warning=No clang version directory found in {:?}",
                toolchain_base
            );
            return;
        }
    };

    // Build the path to the darwin lib directory
    let darwin_lib_dir = clang_dir.join("lib").join("darwin");
    let clang_rt_lib = darwin_lib_dir.join("libclang_rt.osx.a");

    if !clang_rt_lib.exists() {
        println!(
            "cargo:warning=clang runtime library not found at {:?}",
            clang_rt_lib
        );
        return;
    }

    // Emit linker directives
    println!("cargo:rustc-link-search=native={}", darwin_lib_dir.display());
    println!("cargo:rustc-link-lib=static=clang_rt.osx");
    println!(
        "cargo:warning=Linked clang runtime from {:?}",
        darwin_lib_dir
    );
}

/// Copy ONNX Runtime DLLs to the binaries directory on Windows.
///
/// The ort-sys crate downloads ONNX Runtime binaries during build, but they need
/// to be bundled with the application for it to run on target machines.
/// This function finds the downloaded DLLs and copies them to src-tauri/binaries/
/// where Tauri's bundle configuration will pick them up.
///
/// Search order:
/// 1. Already in binaries/ directory (from previous build)
/// 2. ORT_DYLIB_PATH environment variable (ort 2.0 runtime path)
/// 3. ORT_LIB_LOCATION environment variable (legacy/manual override)
/// 4. Pre-installed: %LOCALAPPDATA%\Programs\onnxruntime\ (from requirements script)
/// 5. ort-sys 2.0 global cache: %LOCALAPPDATA%\ort\
/// 6. Legacy: target/*/build/ort-sys-*/out/
///
/// NOTE: GPU EMBEDDING DISABLED - This function is currently unused. To re-enable,
/// uncomment the call site in main() and the ort dependencies in Cargo.toml.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
fn copy_onnx_runtime_dlls(manifest_path: &Path) {
    let binaries_dir = manifest_path.join("binaries");
    
    // Create binaries directory if it doesn't exist
    if !binaries_dir.exists() {
        if let Err(e) = fs::create_dir_all(&binaries_dir) {
            println!("cargo:warning=Failed to create binaries directory: {}", e);
            return;
        }
    }
    
    // 0. Check if DLLs are already in binaries/ (from previous build)
    let existing_dll = binaries_dir.join("onnxruntime.dll");
    if existing_dll.exists() {
        println!("cargo:warning=ONNX Runtime DLLs already present in binaries/");
        return;
    }
    
    // 1. Check ORT_DYLIB_PATH (ort 2.0 uses this at runtime)
    if let Ok(ort_dylib_path) = env::var("ORT_DYLIB_PATH") {
        let dylib_path = Path::new(&ort_dylib_path);
        // ORT_DYLIB_PATH points to the DLL file itself, get the parent directory
        if let Some(dll_dir) = dylib_path.parent() {
            if copy_dlls_from_dir(dll_dir, &binaries_dir) {
                println!("cargo:warning=Copied ONNX Runtime DLLs from ORT_DYLIB_PATH");
                return;
            }
        }
    }
    
    // 2. Check ORT_LIB_LOCATION (legacy/manual override)
    if let Ok(ort_lib_dir) = env::var("ORT_LIB_LOCATION") {
        if copy_dlls_from_dir(Path::new(&ort_lib_dir), &binaries_dir) {
            println!("cargo:warning=Copied ONNX Runtime DLLs from ORT_LIB_LOCATION");
            return;
        }
    }
    
    // 3. Check pre-installed location: %LOCALAPPDATA%\Programs\onnxruntime\
    // This is where windows-requirements.ps1 installs ONNX Runtime
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let programs_dir = Path::new(&local_app_data)
            .join("Programs")
            .join("onnxruntime");
        if programs_dir.exists() {
            if copy_dlls_from_dir(&programs_dir, &binaries_dir) {
                println!(
                    "cargo:warning=Copied ONNX Runtime DLLs from Programs: {:?}",
                    programs_dir
                );
                return;
            }
        }
    }
    
    // 5. Check ort-sys 2.0 global cache: %LOCALAPPDATA%\ort\
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let ort_cache_dir = Path::new(&local_app_data).join("ort");
        if let Some(dll_dir) = find_onnx_dlls_recursive(&ort_cache_dir) {
            if copy_dlls_from_dir(&dll_dir, &binaries_dir) {
                println!(
                    "cargo:warning=Copied ONNX Runtime DLLs from ort cache: {:?}",
                    dll_dir
                );
                return;
            }
        }
    }
    
    // 6. Legacy: Search in the target build directory for ort-sys output
    let target_dir = manifest_path.join("target");
    let profiles = ["release", "debug"];
    
    for profile in &profiles {
        let build_dir = target_dir.join(profile).join("build");
        if !build_dir.exists() {
            continue;
        }
        
        // Look for ort-sys-* directories
        if let Ok(entries) = fs::read_dir(&build_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let dir_name = entry.file_name();
                let dir_name_str = dir_name.to_string_lossy();
                
                if dir_name_str.starts_with("ort-sys-") {
                    let out_dir = entry.path().join("out");
                    
                    // Search recursively for DLLs
                    if let Some(dll_dir) = find_onnx_dlls_recursive(&out_dir) {
                        if copy_dlls_from_dir(&dll_dir, &binaries_dir) {
                            println!(
                                "cargo:warning=Copied ONNX Runtime DLLs from target build: {:?}",
                                dll_dir
                            );
                            return;
                        }
                    }
                }
            }
        }
    }
    
    // If we get here on a fresh build, ort-sys may not have downloaded yet.
    // The DLLs will be available after ort-sys runs its build.rs.
    // This is expected on first build - the runtime will find them via ORT_DYLIB_PATH.
    println!("cargo:warning=ONNX Runtime DLLs not found for bundling.");
    println!("cargo:warning=After first successful build, run 'cargo build' again to bundle DLLs.");
    println!("cargo:warning=Or set ORT_DYLIB_PATH to the onnxruntime.dll location.");
}

/// Recursively search for a directory containing ONNX Runtime DLLs
/// NOTE: GPU EMBEDDING DISABLED - This function is currently unused.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
fn find_onnx_dlls_recursive(dir: &Path) -> Option<std::path::PathBuf> {
    if !dir.exists() {
        return None;
    }
    
    // Check if this directory contains onnxruntime*.dll
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();
            
            if file_name_str.starts_with("onnxruntime") && file_name_str.ends_with(".dll") {
                return Some(dir.to_path_buf());
            }
            
            // Recurse into subdirectories
            if entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                if let Some(found) = find_onnx_dlls_recursive(&entry.path()) {
                    return Some(found);
                }
            }
        }
    }
    
    None
}

/// Copy all DLL files from source directory to destination
/// NOTE: GPU EMBEDDING DISABLED - This function is currently unused.
#[cfg(target_os = "windows")]
#[allow(dead_code)]
fn copy_dlls_from_dir(src_dir: &Path, dest_dir: &Path) -> bool {
    if !src_dir.exists() {
        return false;
    }
    
    let mut copied_any = false;
    
    if let Ok(entries) = fs::read_dir(src_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().map(|e| e == "dll").unwrap_or(false) {
                let file_name = entry.file_name();
                let dest_path = dest_dir.join(&file_name);
                
                if let Err(e) = fs::copy(&path, &dest_path) {
                    println!(
                        "cargo:warning=Failed to copy {:?} to {:?}: {}",
                        path, dest_path, e
                    );
                } else {
                    println!("cargo:warning=Copied {:?} to binaries/", file_name);
                    copied_any = true;
                }
            }
        }
    }
    
    copied_any
}
