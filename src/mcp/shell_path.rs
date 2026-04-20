//! Resolve the user's login shell PATH for GUI-launched apps.
//!
//! On macOS/Linux, apps launched from Dock/Finder inherit a minimal PATH
//! that doesn't include tools installed via nvm, fnm, volta, pyenv, etc.
//! This module resolves the full login shell PATH and caches it.

use std::collections::HashSet;
use std::sync::OnceLock;

/// Get the cached login shell PATH. Returns an empty string if resolution fails.
pub fn get_shell_path() -> &'static str {
    static SHELL_PATH: OnceLock<String> = OnceLock::new();
    SHELL_PATH.get_or_init(|| resolve_login_shell_path().unwrap_or_default())
}

#[cfg(unix)]
fn resolve_login_shell_path() -> Option<String> {
    let current_path = std::env::var("PATH").ok();
    let mut best_path: Option<String> = None;

    for shell in shell_candidates() {
        if let Some(candidate_path) = read_path_from_shell(&shell) {
            let merged = merge_paths(&candidate_path, current_path.as_deref());
            if path_score(&merged) > best_path.as_ref().map(|path| path_score(path)).unwrap_or(0) {
                best_path = Some(merged);
            }
        }
    }

    best_path.or(current_path)
}

#[cfg(not(unix))]
fn resolve_login_shell_path() -> Option<String> {
    std::env::var("PATH").ok()
}

#[cfg(unix)]
fn shell_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();

    for candidate in [
        std::env::var("SHELL").ok(),
        Some("zsh".to_string()),
        Some("/bin/zsh".to_string()),
        Some("bash".to_string()),
        Some("/bin/bash".to_string()),
        Some("sh".to_string()),
        Some("/bin/sh".to_string()),
    ]
    .into_iter()
    .flatten()
    {
        if !candidate.is_empty() && seen.insert(candidate.clone()) {
            candidates.push(candidate);
        }
    }

    candidates
}

#[cfg(unix)]
fn read_path_from_shell(shell: &str) -> Option<String> {
    const START: &str = "__AQBOT_PATH_START__";
    const END: &str = "__AQBOT_PATH_END__";

    let output = std::process::Command::new(shell)
        .args([
            "-i",
            "-l",
            "-c",
            &format!("printf '{START}'; printenv PATH; printf '{END}'"),
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    extract_marked_path(&output.stdout, START, END)
}

#[cfg(unix)]
fn extract_marked_path(output: &[u8], start: &str, end: &str) -> Option<String> {
    let stdout = String::from_utf8(output.to_vec()).ok()?;
    let start_idx = stdout.find(start)? + start.len();
    let end_idx = stdout[start_idx..].find(end)? + start_idx;
    let path = stdout[start_idx..end_idx].trim().to_string();

    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

#[cfg(unix)]
fn merge_paths(primary: &str, fallback: Option<&str>) -> String {
    let mut merged = Vec::new();
    let mut seen = HashSet::new();

    for path_list in [Some(primary), fallback] {
        for segment in path_list
            .unwrap_or_default()
            .split(':')
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
        {
            if seen.insert(segment.to_string()) {
                merged.push(segment.to_string());
            }
        }
    }

    merged.join(":")
}

#[cfg(unix)]
fn path_score(path: &str) -> usize {
    path.split(':')
        .filter(|segment| !segment.is_empty())
        .count()
}

#[cfg(all(test, unix))]
mod tests {
    use super::{extract_marked_path, merge_paths, resolve_login_shell_path};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn resolve_login_shell_path_uses_interactive_shell_config() {
        let dir = tempfile::tempdir().unwrap();
        let fake_shell = dir.path().join("fake-shell.sh");
        let fake_node_dir = dir.path().join("bin");
        fs::create_dir_all(&fake_node_dir).unwrap();
        let interactive_path = std::iter::once(fake_node_dir.to_string_lossy().to_string())
            .chain((0..24).map(|index| format!("/tmp/aqbot-shell-{index}")))
            .collect::<Vec<_>>()
            .join(":");

        let script = format!(
            "#!/bin/sh\nmode=plain\nfor arg in \"$@\"; do\n  if [ \"$arg\" = \"-i\" ]; then\n    mode=interactive\n  fi\ndone\nif [ \"$mode\" = \"interactive\" ]; then\n  printf '__AQBOT_PATH_START__%s__AQBOT_PATH_END__\\n' '{}:/usr/bin:/bin'\nelse\n  printf '__AQBOT_PATH_START__%s__AQBOT_PATH_END__\\n' '/usr/bin:/bin'\nfi\n",
            interactive_path
        );
        fs::write(&fake_shell, script).unwrap();

        let mut perms = fs::metadata(&fake_shell).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake_shell, perms).unwrap();

        let original_shell = std::env::var_os("SHELL");
        std::env::set_var("SHELL", &fake_shell);

        let resolved = resolve_login_shell_path().unwrap();

        match original_shell {
            Some(shell) => std::env::set_var("SHELL", shell),
            None => std::env::remove_var("SHELL"),
        }

        assert!(
            resolved
                .split(':')
                .any(|segment| segment == fake_node_dir.to_string_lossy()),
            "expected interactive PATH to include {}, got {}",
            fake_node_dir.display(),
            resolved
        );
    }

    #[test]
    fn extract_marked_path_ignores_shell_noise() {
        let output = b"hello\n__AQBOT_PATH_START__/usr/local/bin:/usr/bin__AQBOT_PATH_END__\n";
        let path =
            extract_marked_path(output, "__AQBOT_PATH_START__", "__AQBOT_PATH_END__").unwrap();
        assert_eq!(path, "/usr/local/bin:/usr/bin");
    }

    #[test]
    fn merge_paths_deduplicates_segments() {
        let merged = merge_paths("/opt/bin:/usr/bin", Some("/usr/bin:/bin"));
        assert_eq!(merged, "/opt/bin:/usr/bin:/bin");
    }
}
