// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Set up a custom panic handler that logs to stderr before the default handler runs
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Log the panic with timestamp
        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        eprintln!("\n╔════════════════════════════════════════════════════════════");
        eprintln!("║ 💥 PANIC DETECTED at {}", timestamp);
        eprintln!("╠════════════════════════════════════════════════════════════");
        
        // Extract location if available
        if let Some(location) = panic_info.location() {
            eprintln!("║ Location: {}:{}:{}", location.file(), location.line(), location.column());
        }
        
        // Extract panic message
        if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            eprintln!("║ Message: {}", s);
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            eprintln!("║ Message: {}", s);
        } else {
            eprintln!("║ Message: (unknown panic payload)");
        }
        
        // Print backtrace if enabled
        eprintln!("║");
        eprintln!("║ Set RUST_BACKTRACE=1 for a backtrace");
        eprintln!("╚════════════════════════════════════════════════════════════\n");
        
        // Call the default handler (which prints backtrace if RUST_BACKTRACE is set)
        default_hook(panic_info);
    }));

    plugable_chat_lib::run()
}
