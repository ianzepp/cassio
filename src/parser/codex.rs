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
use crate::training::{
    ParsedSession, TrainingEvent, TrainingMetadata, TrainingSession, TrainingSource,
    hash_named_chunks, next_event_id, training_stats_from_session,
};

/// Parser for OpenAI Codex `rollout-*.jsonl` session logs.
pub struct CodexParser;

impl Parser for CodexParser {
    fn parse_export(&self, path: &Path) -> Result<ParsedSession, CassioError> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;
        parse_lines(
            lines.into_iter(),
            path.to_string_lossy().to_string(),
            path.parent()
                .map(|parent| parent.to_string_lossy().to_string()),
        )
    }
}

impl CodexParser {
    /// Parse a Codex session from an arbitrary line iterator.
    ///
    /// WHY: Same rationale as `ClaudeParser::parse_from_lines` — testability
    /// without filesystem access, and stdin reuse.
    pub fn parse_from_lines<I: Iterator<Item = String>>(lines: I) -> Result<Session, CassioError> {
        Ok(parse_lines(lines, "stdin".to_string(), None)?.session)
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
fn parse_lines<I: Iterator<Item = String>>(
    lines: I,
    source_path: String,
    source_root: Option<String>,
) -> Result<ParsedSession, CassioError> {
    let mut metadata: Option<SessionMetadata> = None;
    let mut messages: Vec<Message> = Vec::new();
    let mut stats = SessionStats::default();
    let mut current_model: Option<String> = None;
    let mut models_seen: Vec<String> = Vec::new();
    // WHY: Maps call_id → (function_name, args_json_string) so that when the
    // function_call_output arrives, we can reconstruct a readable summary.
    let mut pending_functions: HashMap<String, (String, String)> = HashMap::new();
    let mut first_timestamp: Option<DateTime<Utc>> = None;
    let mut last_timestamp: Option<DateTime<Utc>> = None;
    let mut training_events: Vec<TrainingEvent> = Vec::new();
    let mut sequence: u64 = 0;
    let mut line_count: u64 = 0;
    let mut hash_chunks: Vec<(String, String)> = Vec::new();

    for (line_index, line) in lines.enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        line_count += 1;
        hash_chunks.push((format!("jsonl:{}", line_index + 1), line.clone()));

        let record: CodexRecord = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let source_ref = format!("jsonl:{}", line_index + 1);

        let ts = record.timestamp.parse::<DateTime<Utc>>().ok();
        if let Some(t) = ts {
            if first_timestamp.is_none() {
                first_timestamp = Some(t);
            }
            last_timestamp = Some(t);
        }

        match record.record_type.as_str() {
            "session_meta" => {
                let session_id = record
                    .payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cwd = record
                    .payload
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cli_version = record
                    .payload
                    .get("cli_version")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let git_branch = record
                    .payload
                    .get("git")
                    .and_then(|g| g.get("branch"))
                    .and_then(|b| b.as_str())
                    .map(|s| s.to_string());
                let payload_ts = record
                    .payload
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<DateTime<Utc>>().ok());

                metadata = Some(SessionMetadata {
                    session_id,
                    tool: Tool::Codex,
                    project_path: cwd,
                    started_at: payload_ts.or(ts).unwrap_or_else(Utc::now),
                    session_kind: SessionKind::Uncertain,
                    version: cli_version,
                    git_branch,
                    model: None,
                    title: None,
                });
            }
            "response_item" => {
                let payload_type = record
                    .payload
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                match payload_type {
                    "message" => {
                        let role = record
                            .payload
                            .get("role")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if role == "user" {
                            // Skip - duplicated from event_msg
                            continue;
                        }
                        if role == "assistant"
                            && let Some(content) =
                                record.payload.get("content").and_then(|c| c.as_array())
                        {
                            let mut blocks = Vec::new();
                            let mut has_text = false;
                            for block in content {
                                let bt = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                                if bt == "output_text" {
                                    let text = block
                                        .get("text")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("")
                                        .trim();
                                    if !text.is_empty() {
                                        blocks.push(ContentBlock::Text {
                                            text: text.to_string(),
                                        });
                                        has_text = true;
                                        sequence += 1;
                                        training_events.push(TrainingEvent {
                                            event_id: next_event_id(sequence),
                                            sequence,
                                            timestamp: ts,
                                            role: Some("assistant".to_string()),
                                            event_kind: "message".to_string(),
                                            model: current_model.clone(),
                                            raw_text: Some(text.to_string()),
                                            sanitized_text: None,
                                            tool_name: None,
                                            tool_call_id: None,
                                            tool_input_raw: None,
                                            tool_input_sanitized: None,
                                            tool_output_raw: None,
                                            tool_output_sanitized: None,
                                            usage: None,
                                            source_record_refs: vec![source_ref.clone()],
                                        });
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
                    "function_call" => {
                        let call_id = record
                            .payload
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = record
                            .payload
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args = record
                            .payload
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}")
                            .to_string();
                        if !call_id.is_empty() {
                            pending_functions.insert(call_id, (name, args));
                        }
                        let tool_name = record
                            .payload
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let args_json = record
                            .payload
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}");
                        sequence += 1;
                        training_events.push(TrainingEvent {
                            event_id: next_event_id(sequence),
                            sequence,
                            timestamp: ts,
                            role: Some("assistant".to_string()),
                            event_kind: "tool_call".to_string(),
                            model: current_model.clone(),
                            raw_text: None,
                            sanitized_text: None,
                            tool_name: Some(tool_name),
                            tool_call_id: record
                                .payload
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .map(|value| value.to_string()),
                            tool_input_raw: serde_json::from_str(args_json).ok(),
                            tool_input_sanitized: None,
                            tool_output_raw: None,
                            tool_output_sanitized: None,
                            usage: None,
                            source_record_refs: vec![source_ref.clone()],
                        });
                    }
                    "function_call_output" => {
                        let call_id = record
                            .payload
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if let Some((name, args_json)) = pending_functions.remove(call_id) {
                            stats.tool_calls += 1;

                            let output = record
                                .payload
                                .get("output")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let is_error = serde_json::from_str::<Value>(output)
                                .ok()
                                .and_then(|v| v.get("exit_code")?.as_i64())
                                .is_some_and(|code| code != 0);
                            if is_error {
                                stats.tool_errors += 1;
                            }

                            // Track file operations from shell commands
                            if name == "shell"
                                && let Ok(args) = serde_json::from_str::<Value>(&args_json)
                            {
                                let cmd = args
                                    .get("command")
                                    .map(|c| {
                                        if let Some(arr) = c.as_array() {
                                            arr.iter()
                                                .filter_map(|v| v.as_str())
                                                .collect::<Vec<_>>()
                                                .join(" ")
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
                                        let path = rest.trim_start_matches(['\'', '"']);
                                        let end = path
                                            .find(|c: char| {
                                                c.is_whitespace()
                                                    || c == '\''
                                                    || c == '"'
                                                    || c == '|'
                                                    || c == '>'
                                            })
                                            .unwrap_or(path.len());
                                        if end > 0 {
                                            stats.files_read.insert(path[..end].to_string());
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
                                    name: name.clone(),
                                    success: !is_error,
                                    summary,
                                }],
                                usage: None,
                            });
                            sequence += 1;
                            training_events.push(TrainingEvent {
                                event_id: next_event_id(sequence),
                                sequence,
                                timestamp: ts,
                                role: Some("assistant".to_string()),
                                event_kind: "tool_result".to_string(),
                                model: current_model.clone(),
                                raw_text: None,
                                sanitized_text: None,
                                tool_name: Some(name),
                                tool_call_id: Some(call_id.to_string()),
                                tool_input_raw: serde_json::from_str(&args_json).ok(),
                                tool_input_sanitized: None,
                                tool_output_raw: Some(Value::String(output.to_string())),
                                tool_output_sanitized: None,
                                usage: None,
                                source_record_refs: vec![source_ref.clone()],
                            });
                        }
                    }
                    "reasoning" => {
                        sequence += 1;
                        let raw_text = serde_json::to_string(&record.payload).unwrap_or_default();
                        training_events.push(TrainingEvent {
                            event_id: next_event_id(sequence),
                            sequence,
                            timestamp: ts,
                            role: Some("assistant".to_string()),
                            event_kind: "message".to_string(),
                            model: current_model.clone(),
                            raw_text: Some(raw_text.clone()),
                            sanitized_text: None,
                            tool_name: None,
                            tool_call_id: None,
                            tool_input_raw: None,
                            tool_input_sanitized: None,
                            tool_output_raw: None,
                            tool_output_sanitized: None,
                            usage: None,
                            source_record_refs: vec![source_ref.clone()],
                        });
                    }
                    _ => {}
                }
            }
            "event_msg" => {
                let payload_type = record
                    .payload
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if payload_type == "token_count" {
                    if let Some(info) = record.payload.get("info")
                        && let Some(total) = info.get("total_token_usage")
                    {
                        let input = total
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let cached = total
                            .get("cached_input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let output = total
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let reasoning = total
                            .get("reasoning_output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        // total_token_usage is cumulative, so always overwrite with the latest
                        stats.total_tokens = TokenUsage {
                            input_tokens: input,
                            output_tokens: output + reasoning,
                            cache_read_tokens: cached,
                            cache_creation_tokens: 0,
                        };
                    }
                } else if payload_type == "user_message"
                    && let Some(msg) = record.payload.get("message").and_then(|v| v.as_str())
                {
                    sequence += 1;
                    training_events.push(TrainingEvent {
                        event_id: next_event_id(sequence),
                        sequence,
                        timestamp: ts,
                        role: Some("user".to_string()),
                        event_kind: "message".to_string(),
                        model: None,
                        raw_text: Some(msg.to_string()),
                        sanitized_text: None,
                        tool_name: None,
                        tool_call_id: None,
                        tool_input_raw: None,
                        tool_input_sanitized: None,
                        tool_output_raw: None,
                        tool_output_sanitized: None,
                        usage: None,
                        source_record_refs: vec![source_ref.clone()],
                    });
                    // Clean up message - remove context blocks and file refs
                    let mut text = msg.to_string();
                    // Remove <context ref="...">...</context>
                    while let Some(start) = text.find("<context ref=\"") {
                        if let Some(end) = text[start..].find("</context>") {
                            text = format!(
                                "{}{}",
                                &text[..start],
                                &text[start + end + "</context>".len()..]
                            );
                        } else {
                            break;
                        }
                    }
                    // Remove [@file](url) references
                    while let Some(start) = text.find("[@") {
                        if let Some(paren_end) = text[start..].find(')') {
                            text = format!(
                                "{}{}",
                                &text[..start],
                                text[start + paren_end + 1..].trim_start()
                            );
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
            "turn_context" => {
                let model = record.payload.get("model").and_then(|v| v.as_str());
                if let Some(m) = model
                    && current_model.as_deref() != Some(m)
                {
                    current_model = Some(m.to_string());
                    if !models_seen.iter().any(|seen| seen == m) {
                        models_seen.push(m.to_string());
                    }
                    messages.push(Message {
                        role: Role::System,
                        timestamp: ts,
                        model: Some(m.to_string()),
                        content: vec![ContentBlock::ModelChange {
                            model: m.to_string(),
                        }],
                        usage: None,
                    });
                    sequence += 1;
                    training_events.push(TrainingEvent {
                        event_id: next_event_id(sequence),
                        sequence,
                        timestamp: ts,
                        role: Some("system".to_string()),
                        event_kind: "model_change".to_string(),
                        model: Some(m.to_string()),
                        raw_text: None,
                        sanitized_text: None,
                        tool_name: None,
                        tool_call_id: None,
                        tool_input_raw: None,
                        tool_input_sanitized: None,
                        tool_output_raw: None,
                        tool_output_sanitized: None,
                        usage: None,
                        source_record_refs: vec![source_ref.clone()],
                    });
                }
            }
            _ => {}
        }
    }

    let mut meta =
        metadata.ok_or_else(|| CassioError::Other("No session_meta record found".into()))?;
    meta.model = current_model;
    meta.session_kind = classify_session_kind(&messages);

    if let (Some(first), Some(last)) = (first_timestamp, last_timestamp) {
        let dur = (last - first).num_seconds();
        if dur >= 0 {
            stats.duration_seconds = Some(dur);
        }
    }

    let session = Session {
        metadata: meta,
        messages,
        stats,
    };
    let training_metadata = TrainingMetadata {
        project_path_raw: session.metadata.project_path.clone(),
        project_path_sanitized: session.metadata.project_path.clone(),
        started_at: session.metadata.started_at,
        ended_at: last_timestamp,
        git_branch: session.metadata.git_branch.clone(),
        title: session.metadata.title.clone(),
        session_kind: session.metadata.session_kind.to_string(),
        models_seen,
        version: session.metadata.version.clone(),
    };
    let source = TrainingSource {
        tool: session.metadata.tool.to_string(),
        source_path,
        session_id: session.metadata.session_id.clone(),
        source_hash: hash_named_chunks(hash_chunks),
        source_record_count: Some(line_count),
        source_format: Some("jsonl".to_string()),
        source_root,
    };
    let mut training = TrainingSession::new(
        "codex.v1",
        source,
        training_metadata,
        training_stats_from_session(&session.stats),
    );
    for event in training_events {
        training.push_event(event);
    }

    Ok(ParsedSession { session, training })
}

/// Convert a Codex function name and JSON arguments string into a compact summary.
///
/// Mirrors `format_tool_input` in the Claude parser but uses Codex's function
/// naming conventions (`shell`, `read_file`, `write_file`, `update_plan`).
pub(crate) fn format_codex_function(name: &str, args_json: &str) -> String {
    let args: Value = serde_json::from_str(args_json).unwrap_or(Value::Object(Default::default()));

    match name {
        "shell" => {
            let cmd = args
                .get("command")
                .map(|c| {
                    if let Some(arr) = c.as_array() {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    } else {
                        c.as_str().unwrap_or("").to_string()
                    }
                })
                .unwrap_or_default();
            let truncated = if cmd.len() > 200 {
                format!("{}...", super::truncate(&cmd, 200))
            } else {
                cmd
            };
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
                let summary: String = plan
                    .iter()
                    .filter_map(|s| {
                        let step = s.get("step").and_then(|v| v.as_str())?;
                        let status = s.get("status").and_then(|v| v.as_str())?;
                        Some(format!("{status}: {step}"))
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                if summary.len() > 150 {
                    format!("{}...", super::truncate(&summary, 150))
                } else {
                    summary
                }
            } else {
                let s = serde_json::to_string(&args).unwrap_or_default();
                if s.len() > 150 {
                    format!("{}...", super::truncate(&s, 150))
                } else {
                    s
                }
            }
        }
        _ => {
            let s = serde_json::to_string(&args).unwrap_or_default();
            if s.len() > 150 {
                format!("{}...", super::truncate(&s, 150))
            } else {
                s
            }
        }
    }
}

#[cfg(test)]
#[path = "codex_test.rs"]
mod tests;
