//! Source discovery: finding tool log directories and individual session files.
//!
//! # Architecture overview
//!
//! Discovery is the first step in every pipeline run. It answers two questions:
//! 1. **Where are the session logs?** — `discover_all_sources*` functions return
//!    `(Tool, PathBuf)` pairs for each installed AI tool.
//! 2. **Which files within a directory contain sessions?** — `find_session_files`
//!    walks a directory and returns the parseable session files within it.
//!
//! This module deliberately knows nothing about parsing. It only produces file
//! paths; the caller chooses which parser to hand them to.
//!
//! # File format heuristics
//!
//! Each tool has a distinct file layout:
//! - **Claude Code**: any `*.jsonl` file (excluding `.bak` variants)
//! - **Codex**: only `rollout-*.jsonl` files (other `.jsonl` files are internal state)
//! - **Hermes**: `state.db` sessions plus legacy `~/.hermes/sessions/*.{json,jsonl}`
//! - **OpenCode**: session IDs are directories named `ses_*` under `message/`
//! - **pi**: any `*.jsonl` file under `~/.pi/agent/sessions/`
//! - **Grok**: `chat_history.jsonl` under `~/.grok/sessions/<project>/<session-id>/`
//! - **Cursor**: `*.jsonl` under `~/.cursor/projects/**/agent-transcripts/`
//!
//! # TRADE-OFFS
//!
//! Auto-detection (`tool = None`) uses directory path string matching rather than
//! content inspection, which is fast but fragile if a user places log directories
//! at unexpected locations. Explicit tool hints should always be preferred when
//! the tool is known.
//!
//! Output path derivation for OpenCode reads from the session JSON on disk rather
//! than deriving from the file path, because OpenCode session IDs are opaque UUIDs
//! with no timestamp. This means the first time a batch runs it pays a small extra
//! disk read per OpenCode session. The `main.rs` handles this as a special case.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{Datelike, Timelike};
use rusqlite::{Connection, OpenFlags};
use walkdir::WalkDir;

use crate::ast::Tool;
use crate::config::SourcesConfig;

const ALL_TOOLS: [Tool; 8] = [
    Tool::Claude,
    Tool::ClaudeDesktop,
    Tool::Codex,
    Tool::Hermes,
    Tool::OpenCode,
    Tool::Pi,
    Tool::Grok,
    Tool::Cursor,
];

/// Return the default log directory for a tool, or `None` if it does not exist.
///
/// WHY: Returning `None` rather than an error when a directory is absent lets
/// `discover_all_sources` skip tools that are not installed on this machine
/// without treating that as a failure.
pub fn default_source_path(tool: Tool) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let path = match tool {
        Tool::Claude => home.join(".claude/projects"),
        Tool::ClaudeDesktop => {
            home.join("Library/Application Support/Claude/local-agent-mode-sessions")
        }
        Tool::Codex => home.join(".codex/sessions"),
        Tool::Hermes => home.join(".hermes"),
        Tool::OpenCode => home.join(".local/share/opencode/storage"),
        Tool::Pi => home.join(".pi/agent/sessions"),
        Tool::Grok => home.join(".grok/sessions"),
        Tool::Cursor => home.join(".cursor/projects"),
    };
    if path.exists() { Some(path) } else { None }
}

/// Return all tool source directories that exist on this machine.
///
/// Checks all known tools in a fixed order. Tools whose default directory does not
/// exist are silently skipped.
pub fn discover_all_sources() -> Vec<(Tool, PathBuf)> {
    ALL_TOOLS
        .iter()
        .filter_map(|&tool| default_source_path(tool).map(|p| (tool, p)))
        .collect()
}

/// Return all tool source directories, preferring config-specified paths over defaults.
///
/// For each known tool: if the config specifies a path and it exists, use it;
/// otherwise fall back to `default_source_path`. Tools with no existing directory
/// are omitted from the result.
///
/// WHY: This allows power users to relocate logs without breaking the `--all` mode.
/// The config path wins only when it exists — a misconfigured path degrades to the
/// default rather than failing the whole discovery step.
pub fn discover_all_sources_with_config(sources: &Option<SourcesConfig>) -> Vec<(Tool, PathBuf)> {
    ALL_TOOLS
        .iter()
        .filter_map(|&tool| {
            // Try config path first, then default
            let config_path = sources.as_ref().and_then(|s| match tool {
                Tool::Claude => s.claude_path(),
                Tool::ClaudeDesktop => s.claude_desktop_path(),
                Tool::Codex => s.codex_path(),
                Tool::Hermes => s.hermes_path(),
                Tool::OpenCode => s.opencode_path(),
                Tool::Pi => s.pi_path(),
                Tool::Grok => s.grok_path(),
                Tool::Cursor => s.cursor_path(),
            });
            let path = config_path
                .filter(|p| p.exists())
                .or_else(|| default_source_path(tool));
            path.map(|p| (tool, p))
        })
        .collect()
}

/// Walk a directory and return all parseable session files within it.
///
/// When `tool` is provided, uses the appropriate file-selection heuristic for that
/// tool. When `tool` is `None`, auto-detects based on directory path substrings.
///
/// Each returned pair carries the `Tool` tag so callers can select the right parser
/// without re-inspecting the path.
pub fn find_session_files(dir: &Path, tool: Option<Tool>) -> Vec<(Tool, PathBuf)> {
    let mut results = Vec::new();

    match tool {
        Some(tool @ Tool::Claude) | Some(tool @ Tool::ClaudeDesktop) => {
            find_claude_files(dir, &mut results, tool);
        }
        Some(Tool::Codex) => {
            find_codex_files(dir, &mut results);
        }
        Some(Tool::Hermes) => {
            find_hermes_sessions(dir, &mut results);
        }
        Some(Tool::OpenCode) => {
            find_opencode_sessions(dir, &mut results);
        }
        Some(Tool::Pi) => {
            find_pi_files(dir, &mut results);
        }
        Some(Tool::Grok) => {
            find_grok_files(dir, &mut results);
        }
        Some(Tool::Cursor) => {
            find_cursor_files(dir, &mut results);
        }
        None => {
            // Auto-detect based on directory content
            let dir_str = dir.to_string_lossy();
            if dir_str.contains(".codex") || dir_str.contains("codex") {
                find_codex_files(dir, &mut results);
            } else if dir_str.contains(".hermes") || dir_str.contains("hermes") {
                find_hermes_sessions(dir, &mut results);
            } else if dir_str.contains("opencode") {
                find_opencode_sessions(dir, &mut results);
            } else if dir_str.contains(".pi/agent/sessions")
                || dir_str.contains("/pi/agent/sessions")
            {
                find_pi_files(dir, &mut results);
            } else if dir_str.contains(".grok/sessions") || dir_str.contains("/.grok/sessions") {
                find_grok_files(dir, &mut results);
            } else if dir_str.contains("agent-transcripts") {
                find_cursor_files(dir, &mut results);
            } else if dir_str.contains("local-agent-mode-sessions") {
                find_claude_files(dir, &mut results, Tool::ClaudeDesktop);
            } else {
                find_claude_files(dir, &mut results, Tool::Claude);
            }
        }
    }

    results
}

/// Collect all Claude/Claude Desktop session files under `dir`.
///
/// Claude Code stores one session per `.jsonl` file. `.bak` variants are
/// leftovers from interrupted writes and must not be parsed.
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

/// Collect Codex rollout files under `dir`.
///
/// WHY: Codex places other JSONL files (e.g., internal state) alongside session
/// files. Only `rollout-*.jsonl` files are actual session transcripts.
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

/// Collect Hermes sessions from SQLite state and legacy on-disk session files.
///
/// Hermes' current store keeps many conversations in one SQLite database. Cassio's
/// batch path expects one path per output transcript, so DB rows are represented as
/// virtual child paths: `state.db/<session_id>`.
fn find_hermes_sessions(dir: &Path, results: &mut Vec<(Tool, PathBuf)>) {
    let mut roots = Vec::new();
    collect_hermes_roots(dir, &mut roots);
    if roots.is_empty() {
        roots.push(dir.to_path_buf());
    }

    for root in roots {
        find_hermes_sessions_in_root(&root, results);
    }
}

fn collect_hermes_roots(dir: &Path, roots: &mut Vec<PathBuf>) {
    for entry in WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !(name == ".git" || name == "node_modules" || name == "venv")
        })
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("state.db").is_file() || path.join("sessions").is_dir() {
            roots.push(path.to_path_buf());
        }
    }
}

fn find_hermes_sessions_in_root(dir: &Path, results: &mut Vec<(Tool, PathBuf)>) {
    let mut db_session_ids = HashSet::new();
    let db_path = dir.join("state.db");
    if db_path.is_file()
        && let Ok(conn) = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        && let Ok(mut stmt) = conn.prepare(
            "SELECT id FROM sessions \
             WHERE EXISTS (SELECT 1 FROM messages WHERE messages.session_id = sessions.id) \
             ORDER BY started_at",
        )
        && let Ok(rows) = stmt.query_map([], |row| row.get::<_, String>(0))
    {
        for id in rows.filter_map(|r| r.ok()) {
            db_session_ids.insert(id.clone());
            results.push((Tool::Hermes, db_path.join(id)));
        }
    }

    let sessions_dir = dir.join("sessions");
    if sessions_dir.is_dir() {
        for entry in WalkDir::new(&sessions_dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Some(id) = legacy_hermes_session_id(name)
                && db_session_ids.contains(id)
            {
                continue;
            }
            let is_json =
                path.extension().is_some_and(|e| e == "json") && name.starts_with("session_");
            let is_jsonl = path.extension().is_some_and(|e| e == "jsonl");
            if is_json || is_jsonl {
                results.push((Tool::Hermes, path.to_path_buf()));
            }
        }
    }
}

fn legacy_hermes_session_id(name: &str) -> Option<&str> {
    name.strip_prefix("session_")
        .and_then(|s| s.strip_suffix(".json"))
        .or_else(|| name.strip_suffix(".jsonl"))
}

/// Collect OpenCode session directories under `dir`.
///
/// OpenCode's storage layout is fragmented: session metadata lives under
/// `session/<project_id>/<ses_id>.json`, messages under `message/<ses_id>/<msg_id>.json`,
/// and parts under `part/<msg_id>/<part_id>.json`. The session ID directory under
/// `message/` is the canonical path passed to the parser.
fn find_opencode_sessions(dir: &Path, results: &mut Vec<(Tool, PathBuf)>) {
    let message_dir = dir.join("message");
    if message_dir.is_dir()
        && let Ok(entries) = std::fs::read_dir(&message_dir)
    {
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("ses_") {
                results.push((Tool::OpenCode, entry.path()));
            }
        }
    }
}

fn find_pi_files(dir: &Path, results: &mut Vec<(Tool, PathBuf)>) {
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "jsonl") {
            results.push((Tool::Pi, path.to_path_buf()));
        }
    }
}

fn find_grok_files(dir: &Path, results: &mut Vec<(Tool, PathBuf)>) {
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path
            .file_name()
            .is_some_and(|name| name == "chat_history.jsonl")
        {
            results.push((Tool::Grok, path.to_path_buf()));
        }
    }
}

fn find_cursor_files(dir: &Path, results: &mut Vec<(Tool, PathBuf)>) {
    for entry in WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        let path_str = path.to_string_lossy();
        if path.extension().is_some_and(|e| e == "jsonl")
            && path_str.contains("/agent-transcripts/")
        {
            results.push((Tool::Cursor, path.to_path_buf()));
        }
    }
}

/// Derive the output path `(year-month folder, filename)` for a session file.
///
/// Used in batch mode to organize transcripts into `YYYY-MM/` subdirectories.
/// For OpenCode, this returns a placeholder because the real timestamp requires
/// reading the session JSON — the caller in `main.rs` handles that case separately.
pub fn derive_output_path(tool: Tool, path: &Path) -> (String, String) {
    match tool {
        Tool::Claude | Tool::ClaudeDesktop => derive_claude_output_path(path),
        Tool::Codex => derive_codex_output_path(path),
        Tool::Hermes => derive_hermes_output_path(path),
        Tool::Pi => derive_pi_output_path(path),
        Tool::Grok => derive_grok_output_path(path),
        Tool::Cursor => derive_cursor_output_path(path),
        Tool::OpenCode => {
            // For OpenCode we need the session data; use a placeholder
            ("unknown".to_string(), format!("unknown-{tool}.md"))
        }
    }
}

/// Derive the output path for a Claude session file by reading its first record's timestamp.
///
/// WHY: Claude session filenames are opaque UUIDs with no date component. The
/// timestamp must be extracted from the first JSON record in the file. The resulting
/// path uses `YYYY-MM/YYYY-MM-DDTHH-MM-SS-claude.md` format so transcripts sort
/// chronologically within the output directory.
fn derive_claude_output_path(path: &Path) -> (String, String) {
    // Read first line to get timestamp
    if let Ok(first_line) = read_first_line(path)
        && let Ok(record) = serde_json::from_str::<serde_json::Value>(&first_line)
        && let Some(ts) = record.get("timestamp").and_then(|t| t.as_str())
    {
        let folder = ts.get(..7).unwrap_or("unknown").to_string();
        let safe_ts = if let Some(dot) = ts.find('.') {
            ts[..dot].replace(':', "-")
        } else {
            ts.replace(':', "-").trim_end_matches('Z').to_string()
        };
        return (folder, format!("{safe_ts}-claude.md"));
    }
    ("unknown".to_string(), "unknown-claude.md".to_string())
}

/// Derive the output path for a Codex rollout file from its filename.
///
/// Codex embeds the session timestamp directly in the filename as
/// `rollout-YYYY-MM-DDTHH-MM-SS-<uuid>.jsonl`, so no file I/O is needed.
fn derive_codex_output_path(path: &Path) -> (String, String) {
    // Filename: rollout-YYYY-MM-DDTHH-MM-SS-uuid.jsonl
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Extract timestamp parts
    if let Some(rest) = filename.strip_prefix("rollout-") {
        // rest: 2025-11-11T14-12-49-019a7455-...
        // We need YYYY-MM for folder, YYYY-MM-DDTHH-MM-SS for timestamp
        if rest.len() >= 19 {
            let ts_part = &rest[..19]; // 2025-11-11T14-12-49
            let folder = &ts_part[..7]; // 2025-11
            return (folder.to_string(), format!("{ts_part}-codex.md"));
        }
    }
    ("unknown".to_string(), "unknown-codex.md".to_string())
}

fn derive_hermes_output_path(path: &Path) -> (String, String) {
    let path_str = path.to_string_lossy();
    if let Some((_, id)) = path_str.split_once("state.db/") {
        return derive_hermes_output_path_from_id(id);
    }

    if path.extension().is_some_and(|e| e == "json")
        && let Ok(content) = std::fs::read_to_string(path)
        && let Ok(record) = serde_json::from_str::<serde_json::Value>(&content)
    {
        if let Some(id) = record.get("session_id").and_then(|s| s.as_str()) {
            return derive_hermes_output_path_from_id(id);
        }
        if let Some(ts) = record.get("session_start").and_then(|t| t.as_str()) {
            return derive_hermes_output_path_from_timestamp(ts);
        }
    }

    if let Some(name) = path.file_name().and_then(|n| n.to_str())
        && let Some(id) = legacy_hermes_session_id(name)
    {
        return derive_hermes_output_path_from_id(id);
    }

    ("unknown".to_string(), "unknown-hermes.md".to_string())
}

fn derive_hermes_output_path_from_id(id: &str) -> (String, String) {
    if id.len() >= 15 {
        let year = &id[0..4];
        let month = &id[4..6];
        let day = &id[6..8];
        let hour = &id[9..11];
        let minute = &id[11..13];
        let second = &id[13..15];
        let suffix = sanitize_stem_suffix(id.get(16..).unwrap_or(""));
        let stem = if suffix.is_empty() {
            format!("{year}-{month}-{day}T{hour}-{minute}-{second}-hermes")
        } else {
            format!("{year}-{month}-{day}T{hour}-{minute}-{second}-{suffix}-hermes")
        };
        return (format!("{year}-{month}"), format!("{stem}.md"));
    }
    ("unknown".to_string(), format!("{id}-hermes.md"))
}

fn derive_hermes_output_path_from_timestamp(ts: &str) -> (String, String) {
    if ts.len() >= 19 {
        let folder = ts.get(..7).unwrap_or("unknown").to_string();
        let safe_ts = ts[..19].replace(':', "-");
        return (folder, format!("{safe_ts}-hermes.md"));
    }
    ("unknown".to_string(), "unknown-hermes.md".to_string())
}

fn sanitize_stem_suffix(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

fn derive_grok_output_path(path: &Path) -> (String, String) {
    if let Some(ts) = crate::parser::grok::grok_started_at_from_source(path) {
        let folder = format!("{:04}-{:02}", ts.year(), ts.month());
        let stem = format!(
            "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}-grok.md",
            ts.year(),
            ts.month(),
            ts.day(),
            ts.hour(),
            ts.minute(),
            ts.second()
        );
        return (folder, stem);
    }
    if let Some(id) = crate::parser::grok::grok_session_id_from_source(path) {
        return ("unknown".to_string(), format!("{id}-grok.md"));
    }
    ("unknown".to_string(), "unknown-grok.md".to_string())
}

fn derive_cursor_output_path(path: &Path) -> (String, String) {
    if let Some(ts) = crate::parser::cursor::cursor_started_at_from_source(path) {
        let folder = format!("{:04}-{:02}", ts.year(), ts.month());
        let stem = format!(
            "{:04}-{:02}-{:02}T{:02}-{:02}-{:02}-cursor.md",
            ts.year(),
            ts.month(),
            ts.day(),
            ts.hour(),
            ts.minute(),
            ts.second()
        );
        return (folder, stem);
    }
    if let Some(id) = path.file_stem().and_then(|stem| stem.to_str()) {
        return ("unknown".to_string(), format!("{id}-cursor.md"));
    }
    ("unknown".to_string(), "unknown-cursor.md".to_string())
}

fn derive_pi_output_path(path: &Path) -> (String, String) {
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if filename.len() >= 19 {
        let ts_part = &filename[..19];
        if ts_part.as_bytes().get(4) == Some(&b'-')
            && ts_part.as_bytes().get(7) == Some(&b'-')
            && ts_part.as_bytes().get(10) == Some(&b'T')
        {
            let folder = &ts_part[..7];
            return (folder.to_string(), format!("{ts_part}-pi.md"));
        }
    }
    ("unknown".to_string(), "unknown-pi.md".to_string())
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
#[path = "discover_test.rs"]
mod tests;
