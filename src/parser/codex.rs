//! Parser for OpenAI Codex session logs (`rollout-*.jsonl` format).
//!
//! # System context
//!
//! Codex writes one JSON record per line to `~/.codex/sessions/**/rollout-*.jsonl`.
//! Records use a uniform envelope: `{ "timestamp": "...", "type": "...", "payload": {...} }`.
//! The `type` field determines the shape of `payload`:
//!
//! | Record type    | Purpose                                              |
//! |----------------|------------------------------------------------------|
//! | `session_meta` | One-time header with session ID, cwd, version, git  |
//! | `event_msg`    | User input (`user_message` subtype)                  |
//! | `response_item`| Assistant output, function calls, and outputs        |
//! | `turn_context` | Model name for the upcoming turn                     |
//!
//! # Tool call handling
//!
//! Codex calls them "function calls" rather than "tool calls" (reflecting the
//! OpenAI API's function-calling interface). The interaction spans two records:
//! - `response_item` with `type: "function_call"` — the invocation
//! - `response_item` with `type: "function_call_output"` — the outcome
//!
//! `pending_functions: HashMap<call_id, (name, args_json)>` tracks in-flight calls.
//! Error detection inspects `exit_code` in the output JSON string rather than a
//! boolean flag.
//!
//! # User message cleanup
//!
//! Codex embeds file contents and context blocks directly in the user message string:
//! - `<context ref="...">...</context>` — file or snippet context blocks
//! - `[@filename](url)` — inline file references
//!
//! These are stripped before adding the text to the AST so transcripts contain
//! only the actual user intent.
//!
//! # TRADE-OFFS
//!
//! - `response_item` records with `role: "user"` are skipped because they duplicate
//!   content already captured by the `event_msg` records. Keeping only `event_msg`
//!   avoids double-counting user messages.
//! - File read tracking uses simple string pattern matching on shell commands
//!   (`cat`, `less`, etc.) rather than full command parsing. This catches the most
//!   common cases but will miss reads via pipes or aliases.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::ast::*;
use crate::error::CassioError;
use crate::parser::Parser;

/// Parser for OpenAI Codex `rollout-*.jsonl` session logs.
pub struct CodexParser;

impl Parser for CodexParser {
    fn parse_session(&self, path: &Path) -> Result<Session, CassioError> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        parse_lines(reader.lines().map(|l| l.unwrap_or_default()))
    }
}

impl CodexParser {
    /// Parse a Codex session from an arbitrary line iterator.
    ///
    /// WHY: Same rationale as `ClaudeParser::parse_from_lines` — testability
    /// without filesystem access, and stdin reuse.
    pub fn parse_from_lines<I: Iterator<Item = String>>(lines: I) -> Result<Session, CassioError> {
        parse_lines(lines)
    }
}

/// Top-level envelope shared by every record in a Codex JSONL file.
///
/// WHY: All Codex records use this same three-field envelope regardless of type.
/// Deserializing to a typed struct first, then dispatching on `record_type`, is
/// cleaner than parsing every record as a raw `Value`.
#[derive(Deserialize)]
struct CodexRecord {
    timestamp: String,
    #[serde(rename = "type")]
    record_type: String,
    payload: Value,
}

/// Core parsing routine for Codex JSONL logs.
///
/// PHASE 1: LINE PROCESSING
/// Parse each non-empty line as a `CodexRecord` envelope, track timestamps for
/// duration, and dispatch on `record_type`.
///
/// PHASE 2: RECORD DISPATCH
/// - `session_meta` → initialize `SessionMetadata` from the payload
/// - `response_item` → route to assistant message, function call, or output handling
/// - `event_msg` → extract and clean user messages
/// - `turn_context` → emit `ModelChange` events when the model name shifts
///
/// PHASE 3: FINALIZATION
/// Patch `metadata.model` with the last seen model name, compute duration.
fn parse_lines<I: Iterator<Item = String>>(lines: I) -> Result<Session, CassioError> {
    let mut metadata: Option<SessionMetadata> = None;
    let mut messages: Vec<Message> = Vec::new();
    let mut stats = SessionStats::default();
    let mut current_model: Option<String> = None;
    // WHY: Maps call_id → (function_name, args_json_string) so that when the
    // function_call_output arrives, we can reconstruct a readable summary.
    let mut pending_functions: HashMap<String, (String, String)> = HashMap::new();
    let mut first_timestamp: Option<DateTime<Utc>> = None;
    let mut last_timestamp: Option<DateTime<Utc>> = None;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let record: CodexRecord = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let ts = record.timestamp.parse::<DateTime<Utc>>().ok();
        if let Some(t) = ts {
            if first_timestamp.is_none() {
                first_timestamp = Some(t);
            }
            last_timestamp = Some(t);
        }

        match record.record_type.as_str() {
            "session_meta" => {
                let session_id = record.payload.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let cwd = record.payload.get("cwd").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let cli_version = record.payload.get("cli_version").and_then(|v| v.as_str()).map(|s| s.to_string());
                let git_branch = record.payload.get("git")
                    .and_then(|g| g.get("branch"))
                    .and_then(|b| b.as_str())
                    .map(|s| s.to_string());
                let payload_ts = record.payload.get("timestamp").and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok());

                metadata = Some(SessionMetadata {
                    session_id,
                    tool: Tool::Codex,
                    project_path: cwd,
                    started_at: payload_ts.or(ts).unwrap_or_else(Utc::now),
                    version: cli_version,
                    git_branch,
                    model: None,
                    title: None,
                });
            }
            "response_item" => {
                let payload_type = record.payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match payload_type {
                    "message" => {
                        let role = record.payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
                        if role == "user" {
                            // Skip - duplicated from event_msg
                            continue;
                        }
                        if role == "assistant" {
                            if let Some(content) = record.payload.get("content").and_then(|c| c.as_array()) {
                                let mut blocks = Vec::new();
                                let mut has_text = false;
                                for block in content {
                                    let bt = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                    if bt == "output_text" {
                                        let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("").trim();
                                        if !text.is_empty() {
                                            blocks.push(ContentBlock::Text { text: text.to_string() });
                                            has_text = true;
                                        }
                                    }
                                }
                                if has_text {
                                    stats.assistant_messages += 1;
                                }
                                if !blocks.is_empty() {
                                    messages.push(Message {
                                        role: Role::Assistant,
                                        timestamp: ts,
                                        model: current_model.clone(),
                                        content: blocks,
                                        usage: None,
                                    });
                                }
                            }
                        }
                    }
                    "function_call" => {
                        let call_id = record.payload.get("call_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let name = record.payload.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let args = record.payload.get("arguments").and_then(|v| v.as_str()).unwrap_or("{}").to_string();
                        if !call_id.is_empty() {
                            pending_functions.insert(call_id, (name, args));
                        }
                    }
                    "function_call_output" => {
                        let call_id = record.payload.get("call_id").and_then(|v| v.as_str()).unwrap_or("");
                        if let Some((name, args_json)) = pending_functions.remove(call_id) {
                            stats.tool_calls += 1;

                            let output = record.payload.get("output").and_then(|v| v.as_str()).unwrap_or("");
                            let is_error = serde_json::from_str::<Value>(output)
                                .ok()
                                .and_then(|v| v.get("exit_code")?.as_i64())
                                .is_some_and(|code| code != 0);
                            if is_error {
                                stats.tool_errors += 1;
                            }

                            // Track file operations from shell commands
                            if name == "shell" {
                                if let Ok(args) = serde_json::from_str::<Value>(&args_json) {
                                    let cmd = args.get("command")
                                        .map(|c| {
                                            if let Some(arr) = c.as_array() {
                                                arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" ")
                                            } else {
                                                c.as_str().unwrap_or("").to_string()
                                            }
                                        })
                                        .unwrap_or_default();
                                    // Track read operations
                                    let re_patterns = ["cat ", "less ", "head ", "tail ", "bat "];
                                    for pat in &re_patterns {
                                        if let Some(idx) = cmd.find(pat) {
                                            let rest = &cmd[idx + pat.len()..];
                                            let path = rest.trim_start_matches(|c: char| c == '\'' || c == '"');
                                            let end = path.find(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '|' || c == '>').unwrap_or(path.len());
                                            if end > 0 {
                                                stats.files_read.insert(path[..end].to_string());
                                            }
                                        }
                                    }
                                }
                            }

                            let summary = format_codex_function(&name, &args_json);
                            messages.push(Message {
                                role: Role::Assistant,
                                timestamp: ts,
                                model: current_model.clone(),
                                content: vec![ContentBlock::ToolResult {
                                    tool_use_id: call_id.to_string(),
                                    name,
                                    success: !is_error,
                                    summary,
                                }],
                                usage: None,
                            });
                        }
                    }
                    "reasoning" => {
                        // Skip encrypted reasoning blocks
                    }
                    _ => {}
                }
            }
            "event_msg" => {
                let payload_type = record.payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if payload_type == "user_message" {
                    if let Some(msg) = record.payload.get("message").and_then(|v| v.as_str()) {
                        // Clean up message - remove context blocks and file refs
                        let mut text = msg.to_string();
                        // Remove <context ref="...">...</context>
                        while let Some(start) = text.find("<context ref=\"") {
                            if let Some(end) = text[start..].find("</context>") {
                                text = format!("{}{}", &text[..start], &text[start + end + "</context>".len()..]);
                            } else {
                                break;
                            }
                        }
                        // Remove [@file](url) references
                        while let Some(start) = text.find("[@") {
                            if let Some(paren_end) = text[start..].find(')') {
                                text = format!("{}{}", &text[..start], text[start + paren_end + 1..].trim_start());
                            } else {
                                break;
                            }
                        }
                        let text = text.trim().to_string();
                        if !text.is_empty() {
                            stats.user_messages += 1;
                            messages.push(Message {
                                role: Role::User,
                                timestamp: ts,
                                model: None,
                                content: vec![ContentBlock::Text { text }],
                                usage: None,
                            });
                        }
                    }
                }
            }
            "turn_context" => {
                let model = record.payload.get("model").and_then(|v| v.as_str());
                if let Some(m) = model {
                    if current_model.as_deref() != Some(m) {
                        current_model = Some(m.to_string());
                        messages.push(Message {
                            role: Role::System,
                            timestamp: ts,
                            model: Some(m.to_string()),
                            content: vec![ContentBlock::ModelChange { model: m.to_string() }],
                            usage: None,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    let mut meta = metadata.ok_or_else(|| CassioError::Other("No session_meta record found".into()))?;
    meta.model = current_model;

    if let (Some(first), Some(last)) = (first_timestamp, last_timestamp) {
        let dur = (last - first).num_seconds();
        if dur >= 0 {
            stats.duration_seconds = Some(dur);
        }
    }

    Ok(Session {
        metadata: meta,
        messages,
        stats,
    })
}

/// Convert a Codex function name and JSON arguments string into a compact summary.
///
/// Mirrors `format_tool_input` in the Claude parser but uses Codex's function
/// naming conventions (`shell`, `read_file`, `write_file`, `update_plan`).
pub(crate) fn format_codex_function(name: &str, args_json: &str) -> String {
    let args: Value = serde_json::from_str(args_json).unwrap_or(Value::Object(Default::default()));

    match name {
        "shell" => {
            let cmd = args.get("command")
                .map(|c| {
                    if let Some(arr) = c.as_array() {
                        arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" ")
                    } else {
                        c.as_str().unwrap_or("").to_string()
                    }
                })
                .unwrap_or_default();
            let truncated = if cmd.len() > 200 { format!("{}...", super::truncate(&cmd, 200)) } else { cmd };
            truncated.replace('\n', " ")
        }
        "read_file" => {
            let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            format!("file=\"{path}\"")
        }
        "write_file" => {
            let path = args.get("path").and_then(|p| p.as_str()).unwrap_or("");
            format!("file=\"{path}\"")
        }
        "update_plan" => {
            if let Some(plan) = args.get("plan").and_then(|p| p.as_array()) {
                let summary: String = plan.iter()
                    .filter_map(|s| {
                        let step = s.get("step").and_then(|v| v.as_str())?;
                        let status = s.get("status").and_then(|v| v.as_str())?;
                        Some(format!("{status}: {step}"))
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                if summary.len() > 150 { format!("{}...", super::truncate(&summary, 150)) } else { summary }
            } else {
                let s = serde_json::to_string(&args).unwrap_or_default();
                if s.len() > 150 { format!("{}...", super::truncate(&s, 150)) } else { s }
            }
        }
        _ => {
            let s = serde_json::to_string(&args).unwrap_or_default();
            if s.len() > 150 { format!("{}...", super::truncate(&s, 150)) } else { s }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_record(record_type: &str, ts: &str, payload: Value) -> String {
        serde_json::json!({
            "type": record_type,
            "timestamp": ts,
            "payload": payload,
        }).to_string()
    }

    fn session_meta(id: &str, cwd: &str) -> Value {
        serde_json::json!({
            "id": id,
            "cwd": cwd,
            "cli_version": "1.0.0",
            "git": {"branch": "main"},
        })
    }

    fn user_message(text: &str) -> Value {
        serde_json::json!({
            "type": "user_message",
            "message": text,
        })
    }

    fn assistant_message(text: &str) -> Value {
        serde_json::json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text}],
        })
    }

    #[test]
    fn test_parse_minimal_codex_session() {
        let lines = vec![
            make_record("session_meta", "2025-01-15T10:00:00Z", session_meta("s1", "/proj")),
            make_record("event_msg", "2025-01-15T10:00:01Z", user_message("hello")),
            make_record("response_item", "2025-01-15T10:00:02Z", assistant_message("hi there")),
        ];
        let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.metadata.session_id, "s1");
        assert_eq!(session.metadata.tool, Tool::Codex);
        assert_eq!(session.metadata.project_path, "/proj");
        assert_eq!(session.stats.user_messages, 1);
        assert_eq!(session.stats.assistant_messages, 1);
    }

    #[test]
    fn test_parse_function_call_and_output() {
        let lines = vec![
            make_record("session_meta", "2025-01-15T10:00:00Z", session_meta("s1", "/proj")),
            make_record("response_item", "2025-01-15T10:00:01Z", serde_json::json!({
                "type": "function_call",
                "call_id": "c1",
                "name": "shell",
                "arguments": "{\"command\":\"ls\"}",
            })),
            make_record("response_item", "2025-01-15T10:00:02Z", serde_json::json!({
                "type": "function_call_output",
                "call_id": "c1",
                "output": "{\"exit_code\":0,\"stdout\":\"files\"}",
            })),
        ];
        let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.stats.tool_calls, 1);
        assert_eq!(session.stats.tool_errors, 0);
    }

    #[test]
    fn test_parse_function_error() {
        let lines = vec![
            make_record("session_meta", "2025-01-15T10:00:00Z", session_meta("s1", "/proj")),
            make_record("response_item", "2025-01-15T10:00:01Z", serde_json::json!({
                "type": "function_call",
                "call_id": "c1",
                "name": "shell",
                "arguments": "{}",
            })),
            make_record("response_item", "2025-01-15T10:00:02Z", serde_json::json!({
                "type": "function_call_output",
                "call_id": "c1",
                "output": "{\"exit_code\":1,\"stderr\":\"error\"}",
            })),
        ];
        let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.stats.tool_calls, 1);
        assert_eq!(session.stats.tool_errors, 1);
    }

    #[test]
    fn test_parse_model_change_via_turn_context() {
        let lines = vec![
            make_record("session_meta", "2025-01-15T10:00:00Z", session_meta("s1", "/proj")),
            make_record("turn_context", "2025-01-15T10:00:01Z", serde_json::json!({
                "model": "o3-pro",
            })),
        ];
        let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.metadata.model, Some("o3-pro".to_string()));
        let model_changes: Vec<_> = session.messages.iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::ModelChange { .. }))
            .collect();
        assert_eq!(model_changes.len(), 1);
    }

    #[test]
    fn test_user_message_cleanup_context_blocks() {
        let lines = vec![
            make_record("session_meta", "2025-01-15T10:00:00Z", session_meta("s1", "/proj")),
            make_record("event_msg", "2025-01-15T10:00:01Z",
                user_message("do this <context ref=\"file.rs\">content</context> please")),
        ];
        let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
        let user_texts: Vec<_> = session.messages.iter()
            .filter(|m| m.role == Role::User)
            .flat_map(|m| &m.content)
            .filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None })
            .collect();
        assert_eq!(user_texts.len(), 1);
        assert!(!user_texts[0].contains("<context"));
        assert!(user_texts[0].contains("do this"));
        assert!(user_texts[0].contains("please"));
    }

    #[test]
    fn test_user_message_cleanup_file_refs() {
        let lines = vec![
            make_record("session_meta", "2025-01-15T10:00:00Z", session_meta("s1", "/proj")),
            make_record("event_msg", "2025-01-15T10:00:01Z",
                user_message("fix [@main.rs](http://example.com) now")),
        ];
        let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
        let user_texts: Vec<_> = session.messages.iter()
            .filter(|m| m.role == Role::User)
            .flat_map(|m| &m.content)
            .filter_map(|b| if let ContentBlock::Text { text } = b { Some(text.as_str()) } else { None })
            .collect();
        assert_eq!(user_texts.len(), 1);
        assert!(!user_texts[0].contains("[@"));
    }

    #[test]
    fn test_duration_calculation() {
        let lines = vec![
            make_record("session_meta", "2025-01-15T10:00:00Z", session_meta("s1", "/proj")),
            make_record("event_msg", "2025-01-15T10:05:00Z", user_message("hi")),
        ];
        let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.stats.duration_seconds, Some(300));
    }

    #[test]
    fn test_no_session_meta_errors() {
        let lines = vec![
            make_record("event_msg", "2025-01-15T10:00:00Z", user_message("hello")),
        ];
        let result = CodexParser::parse_from_lines(lines.into_iter());
        assert!(result.is_err());
    }

    // --- format_codex_function tests ---

    #[test]
    fn test_format_codex_function_shell() {
        let result = format_codex_function("shell", r#"{"command":"ls -la"}"#);
        assert_eq!(result, "ls -la");
    }

    #[test]
    fn test_format_codex_function_shell_array() {
        let result = format_codex_function("shell", r#"{"command":["ls","-la"]}"#);
        assert_eq!(result, "ls -la");
    }

    #[test]
    fn test_format_codex_function_read_file() {
        let result = format_codex_function("read_file", r#"{"path":"/foo.rs"}"#);
        assert_eq!(result, "file=\"/foo.rs\"");
    }

    #[test]
    fn test_format_codex_function_write_file() {
        let result = format_codex_function("write_file", r#"{"path":"/bar.rs"}"#);
        assert_eq!(result, "file=\"/bar.rs\"");
    }

    #[test]
    fn test_format_codex_function_update_plan() {
        let result = format_codex_function("update_plan",
            r#"{"plan":[{"step":"do thing","status":"done"},{"step":"next","status":"pending"}]}"#);
        assert!(result.contains("done: do thing"));
        assert!(result.contains("pending: next"));
    }

    #[test]
    fn test_format_codex_function_unknown() {
        let result = format_codex_function("something", r#"{"key":"val"}"#);
        assert!(result.contains("key"));
    }

    #[test]
    fn test_file_read_tracking_from_shell() {
        let lines = vec![
            make_record("session_meta", "2025-01-15T10:00:00Z", session_meta("s1", "/proj")),
            make_record("response_item", "2025-01-15T10:00:01Z", serde_json::json!({
                "type": "function_call",
                "call_id": "c1",
                "name": "shell",
                "arguments": "{\"command\":\"cat /foo/bar.rs\"}",
            })),
            make_record("response_item", "2025-01-15T10:00:02Z", serde_json::json!({
                "type": "function_call_output",
                "call_id": "c1",
                "output": "{\"exit_code\":0}",
            })),
        ];
        let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
        assert!(session.stats.files_read.contains("/foo/bar.rs"));
    }
}
