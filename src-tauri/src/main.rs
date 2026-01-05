// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Install the global crash handler first, before anything else
    plugable_chat_lib::crash_handler::install_crash_handler();

    // Run the application
    plugable_chat_lib::run()
}
