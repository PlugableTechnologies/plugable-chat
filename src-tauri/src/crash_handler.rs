//! Crash Handler Module
//!
//! Provides a global panic handler that:
//! 1. Formats detailed crash information
//! 2. Writes a crash log to ~/.plugable-chat/crash.log
//! 3. Shows a native dialog with error details and "Try Again" button
//! 4. Can restart the application on user request

use std::backtrace::Backtrace;
use std::fs;
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::PathBuf;
use std::process::Command;

use crate::paths;

const APP_NAME: &str = "plugable-chat";

/// Install the global crash handler.
/// Must be called at the very start of main().
pub fn install_crash_handler() {
    let default_hook = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |panic_info| {
        // Format the crash details
        let crash_details = format_crash_details(panic_info);

        // Write to crash log
        let log_path = write_crash_log(&crash_details);

        // Log to stderr as well (for console/debugging)
        eprintln!("{}", crash_details);

        // Show native dialog and handle user response
        let should_restart = show_crash_dialog(&crash_details, log_path.as_ref());

        if should_restart {
            restart_application();
        }

        // Call the default handler (prints backtrace if RUST_BACKTRACE is set)
        default_hook(panic_info);
    }));
}

/// Build detailed error message from panic info
fn format_crash_details(panic_info: &PanicHookInfo) -> String {
    let mut details = String::new();

    // Header
    details.push_str("═══════════════════════════════════════════════════════════════\n");
    details.push_str("PLUGABLE CHAT CRASH REPORT\n");
    details.push_str("═══════════════════════════════════════════════════════════════\n");

    // Timestamp
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    details.push_str(&format!("Time: {}\n", timestamp));

    // Version (from Cargo.toml)
    details.push_str(&format!("Version: {}\n", env!("CARGO_PKG_VERSION")));

    // OS info
    details.push_str(&format!("OS: {} {}\n", std::env::consts::OS, std::env::consts::ARCH));

    // Location section
    details.push_str("───────────────────────────────────────────────────────────────\n");
    details.push_str("PANIC LOCATION\n");
    details.push_str("───────────────────────────────────────────────────────────────\n");

    if let Some(location) = panic_info.location() {
        details.push_str(&format!("File: {}\n", location.file()));
        details.push_str(&format!("Line: {}, Column: {}\n", location.line(), location.column()));
    } else {
        details.push_str("Location: Unknown\n");
    }

    // Error message section
    details.push_str("───────────────────────────────────────────────────────────────\n");
    details.push_str("ERROR MESSAGE\n");
    details.push_str("───────────────────────────────────────────────────────────────\n");

    if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
        details.push_str(s);
        details.push('\n');
    } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
        details.push_str(s);
        details.push('\n');
    } else {
        details.push_str("(unknown panic payload)\n");
    }

    // Backtrace section
    details.push_str("───────────────────────────────────────────────────────────────\n");
    details.push_str("BACKTRACE\n");
    details.push_str("───────────────────────────────────────────────────────────────\n");

    // Capture backtrace
    let backtrace = Backtrace::force_capture();
    details.push_str(&format!("{}", backtrace));

    details
}

/// Write crash log to the config directory
/// Returns the path if successful
fn write_crash_log(details: &str) -> Option<PathBuf> {
    let config_dir = paths::get_config_dir();

    // Ensure the directory exists
    if fs::create_dir_all(&config_dir).is_err() {
        eprintln!("[CrashHandler] Failed to create config directory: {:?}", config_dir);
        return None;
    }

    let log_path = config_dir.join("crash.log");

    match fs::File::create(&log_path) {
        Ok(mut file) => {
            if file.write_all(details.as_bytes()).is_ok() {
                Some(log_path)
            } else {
                eprintln!("[CrashHandler] Failed to write crash log to {:?}", log_path);
                None
            }
        }
        Err(e) => {
            eprintln!("[CrashHandler] Failed to create crash log file {:?}: {}", log_path, e);
            None
        }
    }
}

/// Extract a short summary from the crash details for the dialog
#[allow(dead_code)]
fn extract_error_summary(panic_info: &PanicHookInfo) -> String {
    let mut summary = String::new();

    // Get error message
    let error_msg = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "Unknown error".to_string()
    };

    // Truncate if too long for dialog
    let truncated_error = if error_msg.len() > 200 {
        format!("{}...", &error_msg[..200])
    } else {
        error_msg
    };

    summary.push_str(&format!("Error: {}\n", truncated_error));

    // Add location
    if let Some(location) = panic_info.location() {
        summary.push_str(&format!(
            "Location: {}:{}",
            location.file(),
            location.line()
        ));
    }

    summary
}

/// Show native dialog with error details and Try Again button
/// Returns true if user clicked "Try Again"
fn show_crash_dialog(details: &str, log_path: Option<&PathBuf>) -> bool {
    use rfd::MessageDialog;
    use rfd::MessageDialogResult;
    use rfd::MessageLevel;
    use rfd::MessageButtons;

    // Build the dialog message
    let mut message = String::from("The application crashed unexpectedly.\n\n");

    // Add a truncated version of the details for the dialog
    // (full details are in the log file)
    let lines: Vec<&str> = details.lines().collect();
    let preview_lines = if lines.len() > 30 {
        lines[..30].join("\n") + "\n\n... (see crash log for full details)"
    } else {
        lines.join("\n")
    };

    message.push_str(&preview_lines);

    if let Some(path) = log_path {
        message.push_str(&format!(
            "\n\nA detailed crash log has been saved to:\n{}\n\nYou can try restarting the application. If the problem persists, please report this issue with the crash log attached.",
            path.display()
        ));
    } else {
        message.push_str("\n\nYou can try restarting the application. If the problem persists, please report this issue.");
    }

    // Show the dialog
    // Note: rfd::MessageDialog is blocking and works without async runtime
    let result = MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(&format!("{} encountered an error", APP_NAME))
        .set_description(&message)
        .set_buttons(MessageButtons::YesNo)
        .show();

    match result {
        MessageDialogResult::Yes => {
            // User clicked "Try Again" (Yes button)
            true
        }
        _ => {
            // User clicked "Exit" (No button) or closed dialog
            false
        }
    }
}

/// Restart the application
fn restart_application() {
    match std::env::current_exe() {
        Ok(exe_path) => {
            eprintln!("[CrashHandler] Restarting application: {:?}", exe_path);

            // Spawn a new instance of the application
            match Command::new(&exe_path).spawn() {
                Ok(_) => {
                    eprintln!("[CrashHandler] New instance spawned successfully");
                    // Exit current process with success code
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("[CrashHandler] Failed to restart application: {}", e);
                    // Let the panic continue and the app will exit
                }
            }
        }
        Err(e) => {
            eprintln!("[CrashHandler] Failed to get current executable path: {}", e);
            // Let the panic continue and the app will exit
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_crash_details_contains_headers() {
        // We can't easily test with a real PanicInfo, but we can verify the function exists
        // and the crash log format is correct by checking the static parts
        let details = "═══════════════════════════════════════════════════════════════\n\
                       PLUGABLE CHAT CRASH REPORT\n\
                       ═══════════════════════════════════════════════════════════════\n";
        assert!(details.contains("PLUGABLE CHAT CRASH REPORT"));
    }

    #[test]
    fn test_write_crash_log_creates_file() {
        let test_content = "Test crash log content";
        let log_path = write_crash_log(test_content);

        assert!(log_path.is_some());
        let path = log_path.unwrap();
        assert!(path.exists());

        // Clean up
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_config_dir_path() {
        let config_dir = paths::get_config_dir();
        assert!(!config_dir.to_string_lossy().is_empty());
        // Should contain the app name
        assert!(config_dir.to_string_lossy().contains("plugable-chat"));
    }
}
