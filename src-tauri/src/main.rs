// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Set up ONNX Runtime DLL path for Windows before any ort code runs
    #[cfg(target_os = "windows")]
    setup_onnx_runtime_path();

    // Install the global crash handler first, before anything else
    plugable_chat_lib::crash_handler::install_crash_handler();

    // Run the application
    plugable_chat_lib::run()
}

/// Set ORT_DYLIB_PATH to find the ONNX Runtime DLL on Windows.
/// This must be called before any ort/fastembed code is initialized.
///
/// Search order:
/// 1. Already set by user (respect existing ORT_DYLIB_PATH)
/// 2. Next to the executable (for bundled apps)
/// 3. In the ort-sys global cache (%LOCALAPPDATA%\ort\)
#[cfg(target_os = "windows")]
fn setup_onnx_runtime_path() {
    use std::env;
    use std::path::Path;

    // If already set, respect the user's choice
    if env::var("ORT_DYLIB_PATH").is_ok() {
        return;
    }

    // Try to find onnxruntime.dll in common locations
    let dll_name = "onnxruntime.dll";

    // 1. Check next to the executable (for bundled Tauri apps)
    if let Ok(exe_path) = env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let dll_path = exe_dir.join(dll_name);
            if dll_path.exists() {
                env::set_var("ORT_DYLIB_PATH", &dll_path);
                eprintln!("Set ORT_DYLIB_PATH to {:?}", dll_path);
                return;
            }
        }
    }

    // 2. Check in the ort-sys global cache
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let cache_dir = Path::new(&local_app_data).join("ort");
        if let Some(dll_path) = find_onnxruntime_dll_recursive(&cache_dir) {
            env::set_var("ORT_DYLIB_PATH", &dll_path);
            eprintln!("Set ORT_DYLIB_PATH to {:?}", dll_path);
            return;
        }
    }

    // If not found, ort will try its default search paths
    eprintln!("Warning: Could not find onnxruntime.dll for ORT_DYLIB_PATH");
}

/// Recursively search for onnxruntime.dll in a directory
#[cfg(target_os = "windows")]
fn find_onnxruntime_dll_recursive(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::fs;

    if !dir.exists() {
        return None;
    }

    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();

            // Check for onnxruntime.dll (exact match, not providers_shared)
            if file_name_str == "onnxruntime.dll" {
                return Some(path);
            }

            // Recurse into subdirectories
            if path.is_dir() {
                if let Some(found) = find_onnxruntime_dll_recursive(&path) {
                    return Some(found);
                }
            }
        }
    }

    None
}
