//! Process utilities for cross-platform process spawning.
//!
//! On Windows, console applications spawn with a visible command prompt window by default.
//! This module provides a trait to hide those windows when spawning external processes
//! like the Foundry CLI or MCP servers.

/// Windows creation flag to prevent console window creation
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// Extension trait to hide console windows when spawning processes on Windows.
///
/// On non-Windows platforms, this is a no-op.
///
/// # Example
/// ```ignore
/// use crate::process_utils::HideConsoleWindow;
///
/// let output = std::process::Command::new("some-cli")
///     .args(["arg1", "arg2"])
///     .hide_console_window()
///     .output()?;
/// ```
pub trait HideConsoleWindow {
    /// Configure the command to not create a visible console window on Windows.
    /// On other platforms, this is a no-op.
    fn hide_console_window(&mut self) -> &mut Self;
}

#[cfg(windows)]
impl HideConsoleWindow for std::process::Command {
    fn hide_console_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }
}

#[cfg(windows)]
impl HideConsoleWindow for tokio::process::Command {
    fn hide_console_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }
}

#[cfg(not(windows))]
impl HideConsoleWindow for std::process::Command {
    fn hide_console_window(&mut self) -> &mut Self {
        self
    }
}

#[cfg(not(windows))]
impl HideConsoleWindow for tokio::process::Command {
    fn hide_console_window(&mut self) -> &mut Self {
        self
    }
}
