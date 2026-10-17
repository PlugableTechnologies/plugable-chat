//! Process utilities for cross-platform process spawning.
//!
//! On Windows, console applications spawn with a visible command prompt window by default.
//! This module provides a trait to hide those windows when spawning external processes
//! like the Foundry CLI or MCP servers.

#[cfg(windows)]
use std::os::windows::process::CommandExt;

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
        self.creation_flags(CREATE_NO_WINDOW)
    }
}

#[cfg(windows)]
impl HideConsoleWindow for tokio::process::Command {
    fn hide_console_window(&mut self) -> &mut Self {
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

/// Run `cmd` and collect its output, killing it after `timeout`.
///
/// The preflight report waited on PowerShell CIM queries with no limit; on a busy CI host both reports
/// of a run never appeared, and a hung query on a user's machine would leave "Copy details for
/// support" spinning the same way.
pub fn output_with_timeout(
    cmd: &mut std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    // Read both pipes on their own threads: a child that fills one pipe would otherwise block forever.
    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err_pipe.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("no answer within {} s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    Ok(std::process::Output {
        status,
        stdout: out_reader.join().unwrap_or_default(),
        stderr: err_reader.join().unwrap_or_default(),
    })
}

#[cfg(all(test, unix))]
mod timeout_tests {
    use super::output_with_timeout;
    use std::time::Duration;

    #[test]
    fn a_quick_command_returns_its_output() {
        let out = output_with_timeout(std::process::Command::new("sh").args(["-c", "echo hi; echo err >&2"]), Duration::from_secs(5)).unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "err");
    }

    #[test]
    fn a_hung_command_is_killed_at_the_limit() {
        let started = std::time::Instant::now();
        let err = output_with_timeout(std::process::Command::new("sleep").arg("30"), Duration::from_millis(300)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
