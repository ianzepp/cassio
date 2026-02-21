use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::ast::Tool;
use crate::config::SourcesConfig;

/// Default source paths for each tool.
pub fn default_source_path(tool: Tool) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = match tool {
        Tool::Claude => home.join(".claude/projects"),
        Tool::ClaudeDesktop => home.join("Library/Application Support/Claude/local-agent-mode-sessions"),
        Tool::Codex => home.join(".codex/sessions"),
        Tool::OpenCode => home.join(".local/share/opencode/storage"),
    };
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

/// Discover all available tool source directories.
pub fn discover_all_sources() -> Vec<(Tool, PathBuf)> {
    let tools = [Tool::Claude, Tool::ClaudeDesktop, Tool::Codex, Tool::OpenCode];
    tools
        .iter()
        .filter_map(|&tool| default_source_path(tool).map(|p| (tool, p)))
        .collect()
}

/// Discover sources using config overrides, falling back to defaults.
pub fn discover_all_sources_with_config(sources: &Option<SourcesConfig>) -> Vec<(Tool, PathBuf)> {
    let tools = [Tool::Claude, Tool::ClaudeDesktop, Tool::Codex, Tool::OpenCode];
    tools
        .iter()
        .filter_map(|&tool| {
            // Try config path first, then default
            let config_path = sources.as_ref().and_then(|s| match tool {
                Tool::Claude => s.claude_path(),
                Tool::ClaudeDesktop => s.claude_desktop_path(),
                Tool::Codex => s.codex_path(),
                Tool::OpenCode => s.opencode_path(),
            });
            let path = config_path
                .filter(|p| p.exists())
                .or_else(|| default_source_path(tool));
            path.map(|p| (tool, p))
        })
        .collect()
}

/// Find all parseable session files in a directory.
pub fn find_session_files(dir: &Path, tool: Option<Tool>) -> Vec<(Tool, PathBuf)> {
    let mut results = Vec::new();

    match tool {
        Some(Tool::Claude) | Some(Tool::ClaudeDesktop) => {
            find_claude_files(dir, &mut results, tool.unwrap());
        }
        Some(Tool::Codex) => {
            find_codex_files(dir, &mut results);
        }
        Some(Tool::OpenCode) => {
            find_opencode_sessions(dir, &mut results);
        }
        None => {
            // Auto-detect based on directory content
            let dir_str = dir.to_string_lossy();
            if dir_str.contains(".codex") || dir_str.contains("codex") {
                find_codex_files(dir, &mut results);
            } else if dir_str.contains("opencode") {
                find_opencode_sessions(dir, &mut results);
            } else if dir_str.contains("local-agent-mode-sessions") {
                find_claude_files(dir, &mut results, Tool::ClaudeDesktop);
            } else {
                find_claude_files(dir, &mut results, Tool::Claude);
            }
        }
    }

    results
}

fn find_claude_files(dir: &Path, results: &mut Vec<(Tool, PathBuf)>, tool: Tool) {
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl")
            && !path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().contains(".bak"))
        {
            results.push((tool, path.to_path_buf()));
        }
    }
}

fn find_codex_files(dir: &Path, results: &mut Vec<(Tool, PathBuf)>) {
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl")
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("rollout-"))
        {
            results.push((Tool::Codex, path.to_path_buf()));
        }
    }
}

fn find_opencode_sessions(dir: &Path, results: &mut Vec<(Tool, PathBuf)>) {
    // OpenCode stores sessions under message/<session_id>/
    let message_dir = dir.join("message");
    if message_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&message_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("ses_") {
                    results.push((Tool::OpenCode, entry.path()));
                }
            }
        }
    }
}

/// Derive output path from a session file for batch mode.
/// Returns (year-month folder, timestamp-tool.txt filename).
pub fn derive_output_path(tool: Tool, path: &Path) -> (String, String) {
    match tool {
        Tool::Claude | Tool::ClaudeDesktop => derive_claude_output_path(path),
        Tool::Codex => derive_codex_output_path(path),
        Tool::OpenCode => {
            // For OpenCode we need the session data; use a placeholder
            ("unknown".to_string(), format!("unknown-{tool}.txt"))
        }
    }
}

fn derive_claude_output_path(path: &Path) -> (String, String) {
    // Read first line to get timestamp
    if let Ok(first_line) = read_first_line(path) {
        if let Ok(record) = serde_json::from_str::<serde_json::Value>(&first_line) {
            if let Some(ts) = record.get("timestamp").and_then(|t| t.as_str()) {
                let folder = ts.get(..7).unwrap_or("unknown").to_string();
                let safe_ts = if let Some(dot) = ts.find('.') {
                    ts[..dot].replace(':', "-")
                } else {
                    ts.replace(':', "-").trim_end_matches('Z').to_string()
                };
                return (folder, format!("{safe_ts}-claude.txt"));
            }
        }
    }
    ("unknown".to_string(), "unknown-claude.txt".to_string())
}

fn derive_codex_output_path(path: &Path) -> (String, String) {
    // Filename: rollout-YYYY-MM-DDTHH-MM-SS-uuid.jsonl
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    // Extract timestamp parts
    if let Some(rest) = filename.strip_prefix("rollout-") {
        // rest: 2025-11-11T14-12-49-019a7455-...
        // We need YYYY-MM for folder, YYYY-MM-DDTHH-MM-SS for timestamp
        if rest.len() >= 19 {
            let ts_part = &rest[..19]; // 2025-11-11T14-12-49
            let folder = &ts_part[..7]; // 2025-11
            return (folder.to_string(), format!("{ts_part}-codex.txt"));
        }
    }
    ("unknown".to_string(), "unknown-codex.txt".to_string())
}

fn read_first_line(path: &Path) -> Result<String, std::io::Error> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    Ok(String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_codex_output_path_valid() {
        let path = PathBuf::from("/sessions/rollout-2025-11-11T14-12-49-019a7455-abcd.jsonl");
        let (folder, filename) = derive_codex_output_path(&path);
        assert_eq!(folder, "2025-11");
        assert_eq!(filename, "2025-11-11T14-12-49-codex.txt");
    }

    #[test]
    fn test_derive_codex_output_path_short_filename() {
        let path = PathBuf::from("/sessions/rollout-short.jsonl");
        let (folder, filename) = derive_codex_output_path(&path);
        assert_eq!(folder, "unknown");
        assert_eq!(filename, "unknown-codex.txt");
    }

    #[test]
    fn test_derive_codex_output_path_no_prefix() {
        let path = PathBuf::from("/sessions/something.jsonl");
        let (folder, filename) = derive_codex_output_path(&path);
        assert_eq!(folder, "unknown");
        assert_eq!(filename, "unknown-codex.txt");
    }

    #[test]
    fn test_derive_output_path_opencode_placeholder() {
        let path = PathBuf::from("/storage/message/ses_123");
        let (folder, filename) = derive_output_path(Tool::OpenCode, &path);
        assert_eq!(folder, "unknown");
        assert!(filename.contains("opencode"));
    }
}
