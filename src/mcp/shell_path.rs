//! Resolve the user's login shell PATH for GUI-launched apps.
//!
//! On macOS/Linux, apps launched from Dock/Finder inherit a minimal PATH
//! that doesn't include tools installed via nvm, fnm, volta, pyenv, etc.
//! This module resolves the full login shell PATH and caches it.

use std::sync::OnceLock;

/// Get the cached login shell PATH. Returns an empty string if resolution fails.
pub fn get_shell_path() -> &'static str {
    static SHELL_PATH: OnceLock<String> = OnceLock::new();
    SHELL_PATH.get_or_init(|| resolve_login_shell_path().unwrap_or_default())
}

#[cfg(unix)]
fn resolve_login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let output = std::process::Command::new(&shell)
        .args(["-l", "-c", "echo $PATH"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let path = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

#[cfg(not(unix))]
fn resolve_login_shell_path() -> Option<String> {
    std::env::var("PATH").ok()
}
