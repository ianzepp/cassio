pub mod claude;
pub mod codex;
pub mod opencode;

use std::path::Path;

use crate::ast::Session;
use crate::error::CassioError;

pub trait Parser {
    fn parse_session(&self, path: &Path) -> Result<Session, CassioError>;
}

/// Auto-detect parser based on file content or path hints.
pub fn detect_parser(path: &Path) -> Result<Box<dyn Parser>, CassioError> {
    // Check path-based hints first
    let path_str = path.to_string_lossy();

    if path_str.contains(".codex") || path_str.contains("rollout-") {
        return Ok(Box::new(codex::CodexParser));
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

/// Detect parser from stdin content (first line peek).
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
