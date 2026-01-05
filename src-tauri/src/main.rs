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
/// 3. Pre-installed location (%LOCALAPPDATA%\Programs\onnxruntime\)
/// 4. ort-sys global cache (%LOCALAPPDATA%\ort\)
#[cfg(target_os = "windows")]
fn setup_onnx_runtime_path() {
    use std::env;
    use std::path::Path;

    // If already set, respect the user's choice
    if env::var("ORT_DYLIB_PATH").is_ok() {
        eprintln!("ORT_DYLIB_PATH already set: {:?}", env::var("ORT_DYLIB_PATH"));
        return;
    }

    let dll_name = "onnxruntime.dll";

    // 1. Check next to the executable (for bundled Tauri apps)
    if let Ok(exe_path) = env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let dll_path = exe_dir.join(dll_name);
            if dll_path.exists() {
                env::set_var("ORT_DYLIB_PATH", &dll_path);
                eprintln!("Set ORT_DYLIB_PATH to {:?} (next to exe)", dll_path);
                return;
            }
        }
    }

    // 2. Check pre-installed location (from windows-requirements.ps1)
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let programs_dir = Path::new(&local_app_data)
            .join("Programs")
            .join("onnxruntime")
            .join(dll_name);
        if programs_dir.exists() {
            env::set_var("ORT_DYLIB_PATH", &programs_dir);
            eprintln!("Set ORT_DYLIB_PATH to {:?} (Programs)", programs_dir);
            return;
        }
    }

    // 3. Check ort-sys global cache
    if let Ok(local_app_data) = env::var("LOCALAPPDATA") {
        let cache_dir = Path::new(&local_app_data).join("ort");
        if let Some(dll_path) = find_onnxruntime_dll_recursive(&cache_dir) {
            env::set_var("ORT_DYLIB_PATH", &dll_path);
            eprintln!("Set ORT_DYLIB_PATH to {:?} (ort cache)", dll_path);
            return;
        }
    }

    // If not found, log all the places we checked
    // This is not fatal - the app will continue without embedding features
    eprintln!("Note: ONNX Runtime DLL not found - embedding/search features will be disabled");
    eprintln!("  Checked: next to exe, %LOCALAPPDATA%\\Programs\\onnxruntime\\, %LOCALAPPDATA%\\ort\\");
    eprintln!("  To enable embedding, run requirements.bat or set ORT_DYLIB_PATH environment variable");
    eprintln!("  The app will continue to work for chat, but semantic search will be unavailable.");
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
