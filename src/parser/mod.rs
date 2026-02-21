//! Parser trait, format detection, and shared parsing utilities.
//!
//! # Architecture overview
//!
//! This module sits at the boundary between raw log files and the normalized AST.
//! It defines the `Parser` trait that all tool-specific parsers implement, and
//! provides automatic format detection so callers don't need to know which parser
//! to use for a given file.
//!
//! # Detection strategy
//!
//! Detection uses a layered approach:
//!
//! 1. **Path hints** — directory names like `.codex` or `rollout-` in the path
//!    are strong signals that are checked first, with no I/O cost.
//! 2. **Content peek** — for `.jsonl` files where the path gives no hint, the
//!    first non-empty line is read and checked for format-specific field names.
//! 3. **Default** — unknown `.jsonl` files default to the Claude parser, since
//!    Claude is the most common tool.
//!
//! # TRADE-OFFS
//!
//! - Returning `Box<dyn Parser>` from `detect_parser` rather than an enum avoids
//!   a central match arm for every parser, making it easier to add new parsers.
//!   The allocation cost is negligible for batch processing.
//! - `truncate` is a shared utility here rather than in a `util` module because
//!   every parser module uses it. If parsers grow significantly, a dedicated
//!   utility module would be warranted.

pub mod claude;
pub mod codex;
pub mod opencode;

use std::path::Path;

use crate::ast::Session;
use crate::error::CassioError;

/// Trait implemented by each tool-specific parser.
///
/// WHY: A trait rather than an enum of parsers allows callers to work with a
/// `Box<dyn Parser>` returned by `detect_parser` without knowing which concrete
/// parser they have. Adding a new tool only requires adding a new module and
/// returning it from `detect_parser`.
pub trait Parser {
    /// Parse a session log at `path` into the normalized `Session` AST.
    ///
    /// Each parser is responsible for reading the file, normalizing the tool-specific
    /// schema, and computing session statistics in a single pass.
    fn parse_session(&self, path: &Path) -> Result<Session, CassioError>;
}

/// Select the appropriate parser for a given file path using path hints and content inspection.
///
/// # Error behavior
///
/// Returns `CassioError::UnknownFormat` only for non-`.jsonl` files with no path
/// hints. `.jsonl` files always return a parser (defaulting to Claude).
pub fn detect_parser(path: &Path) -> Result<Box<dyn Parser>, CassioError> {
    // Check path-based hints first
    let path_str = path.to_string_lossy();

    if path_str.contains(".codex") || path_str.contains("rollout-") {
        return Ok(Box::new(codex::CodexParser));
    }

    if path_str.contains("local-agent-mode-sessions") {
        return Ok(Box::new(claude::ClaudeParser));
    }

    if path_str.contains("opencode") {
        return Ok(Box::new(opencode::OpenCodeParser));
    }

    // For .jsonl files, peek at first line to detect format
    if path.extension().is_some_and(|e| e == "jsonl") {
        let first_line = read_first_line(path)?;
        if first_line.contains("\"sessionId\"") {
            return Ok(Box::new(claude::ClaudeParser));
        }
        if first_line.contains("\"session_meta\"") || first_line.contains("\"response_item\"") {
            return Ok(Box::new(codex::CodexParser));
        }
    }

    // Default to Claude parser for .jsonl files
    if path.extension().is_some_and(|e| e == "jsonl") {
        return Ok(Box::new(claude::ClaudeParser));
    }

    Err(CassioError::UnknownFormat(path.to_path_buf()))
}

/// Select a parser based on the first line of stdin content.
///
/// WHY: Stdin mode has no file path to inspect, so detection must rely entirely
/// on content. The same field-name heuristics used in `detect_parser` apply here.
/// Defaults to Claude when no match is found.
pub fn detect_parser_from_content(first_line: &str) -> Box<dyn Parser> {
    if first_line.contains("\"sessionId\"") {
        Box::new(claude::ClaudeParser)
    } else if first_line.contains("\"session_meta\"") || first_line.contains("\"response_item\"") {
        Box::new(codex::CodexParser)
    } else {
        // Default to Claude
        Box::new(claude::ClaudeParser)
    }
}

/// Truncate a string to at most `max` bytes without splitting a UTF-8 codepoint.
///
/// WHY: Tool inputs (shell commands, file contents) can be arbitrarily long. Parsers
/// truncate summaries at fixed byte limits to keep the AST and formatted output
/// readable. Naive byte slicing would corrupt multibyte characters, so this helper
/// walks backwards from the limit to find a safe boundary.
pub(crate) fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn read_first_line(path: &Path) -> Result<String, CassioError> {
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
