use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::types::{Tool, ToolError, ToolInputSchema, ToolResult, ToolUseContext};

const MAX_RESULTS: usize = 100;
const MAX_VISITED_ENTRIES: usize = 20_000;
const MAX_SEARCH_MS: u64 = 3_000;

/// Directories to skip during glob matching.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    ".next",
    "__pycache__",
    ".mypy_cache",
    "target",
    "dist",
    "build",
];

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn description(&self) -> &str {
        "Fast file pattern matching tool. Supports glob patterns like \"**/*.rs\" or \"src/**/*.ts\". Returns matching file paths sorted by modification time."
    }

    fn input_schema(&self) -> ToolInputSchema {
        ToolInputSchema {
            schema_type: "object".to_string(),
            properties: HashMap::from([
                (
                    "pattern".to_string(),
                    json!({
                        "type": "string",
                        "description": "The glob pattern to match files against"
                    }),
                ),
                (
                    "path".to_string(),
                    json!({
                        "type": "string",
                        "description": "The directory to search in (defaults to working directory)"
                    }),
                ),
            ]),
            required: vec!["pattern".to_string()],
            additional_properties: Some(false),
        }
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn call(&self, input: Value, context: &ToolUseContext) -> Result<ToolResult, ToolError> {
        let pattern = input
            .get("pattern")
            .and_then(|p| p.as_str())
            .ok_or_else(|| ToolError::InvalidInput("Missing 'pattern' field".to_string()))?;

        let search_path_value = input
            .get("path")
            .and_then(|p| p.as_str())
            .unwrap_or(&context.working_dir);
        let search_path = resolve_search_path(search_path_value, &context.working_dir);
        let pattern = normalize_pattern(pattern)?;
        let matcher = glob::Pattern::new(&pattern)
            .map_err(|e| ToolError::ExecutionError(format!("Invalid glob pattern: {}", e)))?;
        let (walk_root, max_depth) = walk_root_and_depth(&search_path, &pattern);

        let scan = scan_files(
            &walk_root,
            &search_path,
            &matcher,
            pattern.starts_with('/'),
            max_depth,
            context,
        )?;
        let mut files = scan.files;

        // Sort by modification time (newest first)
        files.sort_by(|a, b| b.1.cmp(&a.1));

        let total = files.len();
        let truncated = scan.truncated || total > MAX_RESULTS;
        let files: Vec<String> = files
            .into_iter()
            .take(MAX_RESULTS)
            .map(|(p, _)| p)
            .collect();

        if files.is_empty() {
            return Ok(ToolResult::text(format!(
                "No files found matching pattern: {}",
                pattern
            )));
        }

        let mut result = files.join("\n");
        if truncated {
            result.push_str(&format!(
                "\n\n(showing first {} matches; search was bounded)",
                files.len().min(MAX_RESULTS)
            ));
        }

        Ok(ToolResult::text(result))
    }
}

struct GlobScan {
    files: Vec<(String, std::time::SystemTime)>,
    truncated: bool,
}

fn normalize_pattern(pattern: &str) -> Result<String, ToolError> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidInput("Empty glob pattern".to_string()));
    }
    Ok(match trimmed {
        "." | "./" => "*".to_string(),
        other => other.to_string(),
    })
}

fn resolve_search_path(input_path: &str, working_dir: &str) -> PathBuf {
    let path = Path::new(input_path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(working_dir).join(path)
    }
}

fn walk_root_and_depth(search_path: &Path, pattern: &str) -> (PathBuf, Option<usize>) {
    let base = extract_glob_base(pattern);
    let root = if pattern.starts_with('/') {
        PathBuf::from(if base.is_empty() { "/" } else { &base })
    } else if base.is_empty() {
        search_path.to_path_buf()
    } else {
        search_path.join(base)
    };
    let remaining = remaining_segments(pattern, &extract_glob_base(pattern));
    let max_depth = if remaining.iter().any(|part| *part == "**") {
        None
    } else {
        Some(remaining.len().max(1))
    };
    (root, max_depth)
}

fn extract_glob_base(pattern: &str) -> String {
    let mut parts = Vec::new();
    for segment in pattern.split('/') {
        if segment.contains('*')
            || segment.contains('?')
            || segment.contains('[')
            || segment.contains('{')
        {
            break;
        }
        parts.push(segment);
    }
    parts.join("/")
}

fn remaining_segments<'a>(pattern: &'a str, base: &str) -> Vec<&'a str> {
    let base_count = base.split('/').filter(|part| !part.is_empty()).count();
    pattern
        .split('/')
        .filter(|part| !part.is_empty())
        .skip(base_count)
        .collect()
}

fn scan_files(
    root: &Path,
    search_path: &Path,
    matcher: &glob::Pattern,
    match_absolute: bool,
    max_depth: Option<usize>,
    context: &ToolUseContext,
) -> Result<GlobScan, ToolError> {
    let deadline = Duration::from_millis(MAX_SEARCH_MS);
    let started = Instant::now();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut files = Vec::new();
    let mut visited = 0usize;
    let mut truncated = false;

    if root.is_file() && path_matches(root, search_path, matcher, match_absolute) {
        let modified = root
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        files.push((root.to_string_lossy().to_string(), modified));
        return Ok(GlobScan { files, truncated });
    }

    while let Some((dir, depth)) = stack.pop() {
        if context.abort_signal.is_cancelled() {
            return Ok(GlobScan {
                files,
                truncated: true,
            });
        }
        if visited >= MAX_VISITED_ENTRIES || started.elapsed() > deadline {
            truncated = true;
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            visited += 1;
            let path = entry.path();
            if path.is_dir() {
                if should_skip_dir(&path) {
                    continue;
                }
                if max_depth.map_or(true, |limit| depth + 1 < limit) {
                    stack.push((path, depth + 1));
                }
            } else if path.is_file() && path_matches(&path, search_path, matcher, match_absolute) {
                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::UNIX_EPOCH);
                files.push((path.to_string_lossy().to_string(), modified));
                if files.len() >= MAX_RESULTS {
                    truncated = true;
                    break;
                }
            }
        }
        if truncated {
            break;
        }
    }

    Ok(GlobScan { files, truncated })
}

fn path_matches(path: &Path, search_path: &Path, matcher: &glob::Pattern, absolute: bool) -> bool {
    if absolute {
        return matcher.matches_path(path);
    }
    path.strip_prefix(search_path)
        .ok()
        .is_some_and(|relative| matcher.matches_path(relative))
}

fn should_skip_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| SKIP_DIRS.contains(&name))
}
