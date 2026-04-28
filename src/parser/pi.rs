//! Parser for pi coding TUI session logs (`~/.pi/agent/sessions/**/*.jsonl`).
//!
//! # System context
//!
//! pi writes one JSON record per line. The stream mixes session metadata,
//! model-change events, and message events with nested message content blocks.
//! The important record types are:
//!
//! - `session` — header with session ID, timestamp, and cwd
//! - `model_change` — active provider/model switch
//! - `message` — user, assistant, and tool-result turns
//!
//! Assistant tool use is split across two message records:
//! - assistant content block with `type: "toolCall"`
//! - later `message.role == "toolResult"` carrying `toolCallId`
//!
//! The parser correlates those using `pending_tools` so transcripts can show a
//! compact tool line and training exports retain the raw input/output linkage.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::ast::*;
use crate::error::CassioError;
use crate::parser::Parser;
use crate::training::{
    ParsedSession, TrainingEvent, TrainingMetadata, TrainingSession, TrainingSource,
    hash_named_chunks, next_event_id, training_stats_from_session,
};

pub struct PiParser;

impl Parser for PiParser {
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

impl PiParser {
    pub fn parse_from_lines<I: Iterator<Item = String>>(lines: I) -> Result<Session, CassioError> {
        Ok(parse_lines(lines, "stdin".to_string(), None)?.session)
    }
}

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
    let mut pending_tools: HashMap<String, (String, Value)> = HashMap::new();
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

        let record: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let source_ref = format!("jsonl:{}", line_index + 1);
        let record_type = record.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ts = record
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(parse_timestamp);

        if let Some(t) = ts {
            if first_timestamp.is_none() {
                first_timestamp = Some(t);
            }
            last_timestamp = Some(t);
        }

        match record_type {
            "session" => {
                let session_id = record
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cwd = record
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                metadata = Some(SessionMetadata {
                    session_id,
                    tool: Tool::Pi,
                    project_path: cwd,
                    started_at: ts.unwrap_or_else(Utc::now),
                    session_kind: SessionKind::Uncertain,
                    version: None,
                    git_branch: None,
                    model: None,
                    title: None,
                });
            }
            "model_change" => {
                let provider = record
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let model_id = record.get("modelId").and_then(|v| v.as_str()).unwrap_or("");
                let model = combine_model(provider, model_id);
                if model.is_empty() || current_model.as_deref() == Some(model.as_str()) {
                    continue;
                }
                current_model = Some(model.clone());
                if !models_seen.iter().any(|seen| seen == &model) {
                    models_seen.push(model.clone());
                }
                messages.push(Message {
                    role: Role::System,
                    timestamp: ts,
                    model: Some(model.clone()),
                    content: vec![ContentBlock::ModelChange {
                        model: model.clone(),
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
                    model: Some(model),
                    raw_text: None,
                    sanitized_text: None,
                    tool_name: None,
                    tool_call_id: None,
                    tool_input_raw: None,
                    tool_input_sanitized: None,
                    tool_output_raw: None,
                    tool_output_sanitized: None,
                    usage: None,
                    source_record_refs: vec![source_ref],
                });
            }
            "message" => {
                let Some(message) = record.get("message") else {
                    continue;
                };
                let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
                match role {
                    "user" => {
                        let mut blocks = Vec::new();
                        let mut has_text = false;
                        if let Some(content) = message.get("content").and_then(|v| v.as_array()) {
                            for block in content {
                                if block.get("type").and_then(|v| v.as_str()) != Some("text") {
                                    continue;
                                }
                                let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                sequence += 1;
                                training_events.push(TrainingEvent {
                                    event_id: next_event_id(sequence),
                                    sequence,
                                    timestamp: ts,
                                    role: Some("user".to_string()),
                                    event_kind: "message".to_string(),
                                    model: None,
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
                                let trimmed = text.trim();
                                if !trimmed.is_empty() {
                                    blocks.push(ContentBlock::Text {
                                        text: trimmed.to_string(),
                                    });
                                    has_text = true;
                                }
                            }
                        }
                        if has_text {
                            stats.user_messages += 1;
                        }
                        if !blocks.is_empty() {
                            messages.push(Message {
                                role: Role::User,
                                timestamp: ts,
                                model: None,
                                content: blocks,
                                usage: None,
                            });
                        }
                    }
                    "assistant" => {
                        let mut blocks = Vec::new();
                        let mut has_text = false;
                        let assistant_model = message
                            .get("model")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| current_model.clone());
                        let usage = message.get("usage").map(parse_usage);
                        if let Some(ref u) = usage {
                            stats.total_tokens.input_tokens += u.input_tokens;
                            stats.total_tokens.output_tokens += u.output_tokens;
                            stats.total_tokens.cache_read_tokens += u.cache_read_tokens;
                            stats.total_tokens.cache_creation_tokens += u.cache_creation_tokens;
                        }

                        if let Some(content) = message.get("content").and_then(|v| v.as_array()) {
                            for block in content {
                                let block_type =
                                    block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                match block_type {
                                    "text" => {
                                        let text = block
                                            .get("text")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        sequence += 1;
                                        training_events.push(TrainingEvent {
                                            event_id: next_event_id(sequence),
                                            sequence,
                                            timestamp: ts,
                                            role: Some("assistant".to_string()),
                                            event_kind: "message".to_string(),
                                            model: assistant_model.clone(),
                                            raw_text: Some(text.to_string()),
                                            sanitized_text: None,
                                            tool_name: None,
                                            tool_call_id: None,
                                            tool_input_raw: None,
                                            tool_input_sanitized: None,
                                            tool_output_raw: None,
                                            tool_output_sanitized: None,
                                            usage: usage
                                                .as_ref()
                                                .map(crate::training::event_usage_from_tokens),
                                            source_record_refs: vec![source_ref.clone()],
                                        });
                                        let trimmed = text.trim();
                                        if !trimmed.is_empty() {
                                            blocks.push(ContentBlock::Text {
                                                text: trimmed.to_string(),
                                            });
                                            has_text = true;
                                        }
                                    }
                                    "thinking" => {
                                        let text = block
                                            .get("thinking")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        blocks.push(ContentBlock::Thinking {
                                            text: text.to_string(),
                                        });
                                        sequence += 1;
                                        training_events.push(TrainingEvent {
                                            event_id: next_event_id(sequence),
                                            sequence,
                                            timestamp: ts,
                                            role: Some("assistant".to_string()),
                                            event_kind: "message".to_string(),
                                            model: assistant_model.clone(),
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
                                    "toolCall" => {
                                        let id = block
                                            .get("id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let name = block
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown")
                                            .to_string();
                                        let input = block
                                            .get("arguments")
                                            .cloned()
                                            .unwrap_or_else(|| Value::Object(Default::default()));
                                        pending_tools
                                            .insert(id.clone(), (name.clone(), input.clone()));
                                        blocks.push(ContentBlock::ToolUse {
                                            id: id.clone(),
                                            name: name.clone(),
                                            input: input.clone(),
                                        });
                                        sequence += 1;
                                        training_events.push(TrainingEvent {
                                            event_id: next_event_id(sequence),
                                            sequence,
                                            timestamp: ts,
                                            role: Some("assistant".to_string()),
                                            event_kind: "tool_call".to_string(),
                                            model: assistant_model.clone(),
                                            raw_text: None,
                                            sanitized_text: None,
                                            tool_name: Some(name),
                                            tool_call_id: Some(id),
                                            tool_input_raw: Some(input),
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
                        }

                        if has_text {
                            stats.assistant_messages += 1;
                        }
                        if !blocks.is_empty() {
                            messages.push(Message {
                                role: Role::Assistant,
                                timestamp: ts,
                                model: assistant_model,
                                content: blocks,
                                usage,
                            });
                        }
                    }
                    "toolResult" => {
                        let tool_call_id = message
                            .get("toolCallId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let fallback_name = message
                            .get("toolName")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let is_error = message
                            .get("isError")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let (name, input) = pending_tools
                            .remove(&tool_call_id)
                            .unwrap_or((fallback_name.clone(), Value::Object(Default::default())));

                        stats.tool_calls += 1;
                        if is_error {
                            stats.tool_errors += 1;
                        }
                        track_file_ops(&mut stats, &name, &input, is_error);

                        let summary = format_pi_tool_input(&name, &input);
                        messages.push(Message {
                            role: Role::Assistant,
                            timestamp: ts,
                            model: current_model.clone(),
                            content: vec![ContentBlock::ToolResult {
                                tool_use_id: tool_call_id.clone(),
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
                            tool_call_id: Some(tool_call_id),
                            tool_input_raw: Some(input),
                            tool_input_sanitized: None,
                            tool_output_raw: Some(tool_result_output(message)),
                            tool_output_sanitized: None,
                            usage: None,
                            source_record_refs: vec![source_ref],
                        });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let mut meta = metadata.ok_or_else(|| CassioError::Other("No session record found".into()))?;
    meta.model = current_model;
    meta.session_kind = classify_session_kind(&messages);
    stats.duration_seconds = match (first_timestamp, last_timestamp) {
        (Some(first), Some(last)) if last >= first => Some((last - first).num_seconds()),
        _ => None,
    };

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
        "pi.v1",
        source,
        training_metadata,
        training_stats_from_session(&session.stats),
    );
    for event in training_events {
        training.push_event(event);
    }

    Ok(ParsedSession { session, training })
}

fn combine_model(provider: &str, model_id: &str) -> String {
    match (provider.is_empty(), model_id.is_empty()) {
        (false, false) => format!("{provider}/{model_id}"),
        (true, false) => model_id.to_string(),
        (false, true) => provider.to_string(),
        (true, true) => String::new(),
    }
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    s.parse::<DateTime<Utc>>().ok()
}

fn parse_usage(value: &Value) -> TokenUsage {
    TokenUsage {
        input_tokens: value.get("input").and_then(|v| v.as_u64()).unwrap_or(0),
        output_tokens: value.get("output").and_then(|v| v.as_u64()).unwrap_or(0),
        cache_read_tokens: value.get("cacheRead").and_then(|v| v.as_u64()).unwrap_or(0),
        cache_creation_tokens: value
            .get("cacheWrite")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    }
}

fn tool_result_output(message: &Value) -> Value {
    let text = message
        .get("content")
        .and_then(|v| v.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    json!({
        "toolName": message.get("toolName").cloned(),
        "isError": message.get("isError").cloned(),
        "details": message.get("details").cloned(),
        "text": text,
    })
}

fn track_file_ops(stats: &mut SessionStats, tool_name: &str, input: &Value, is_error: bool) {
    if is_error {
        return;
    }

    let path = input
        .get("path")
        .and_then(|v| v.as_str())
        .or_else(|| input.get("file_path").and_then(|v| v.as_str()))
        .or_else(|| input.get("filePath").and_then(|v| v.as_str()));
    let Some(path) = path else {
        return;
    };

    match tool_name {
        "read" | "Read" => {
            stats.files_read.insert(path.to_string());
        }
        "write" | "Write" => {
            stats.files_written.insert(path.to_string());
        }
        "edit" | "Edit" => {
            stats.files_edited.insert(path.to_string());
        }
        _ => {}
    }
}

pub(crate) fn format_pi_tool_input(tool_name: &str, input: &Value) -> String {
    match tool_name {
        "bash" | "Bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let truncated = if cmd.len() > 200 {
                format!("{}...", super::truncate(cmd, 200))
            } else {
                cmd.to_string()
            };
            truncated.replace('\n', " \u{21b5} ")
        }
        "read" | "Read" | "write" | "Write" | "edit" | "Edit" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .or_else(|| input.get("file_path").and_then(|v| v.as_str()))
                .or_else(|| input.get("filePath").and_then(|v| v.as_str()))
                .unwrap_or("");
            format!("file=\"{path}\"")
        }
        "web_search" | "WebSearch" => {
            let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
            format!("query=\"{query}\"")
        }
        "web_fetch" | "WebFetch" => {
            let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("");
            format!("url=\"{url}\"")
        }
        _ => {
            let s = serde_json::to_string(input).unwrap_or_default();
            if s.len() > 150 {
                format!("{}...", super::truncate(&s, 150))
            } else {
                s
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(value: Value) -> String {
        value.to_string()
    }

    #[test]
    fn test_parse_minimal_pi_session() {
        let lines = vec![
            line(json!({
                "type": "session",
                "version": 3,
                "id": "pi-1",
                "timestamp": "2026-04-13T09:16:32.078Z",
                "cwd": "/proj"
            })),
            line(json!({
                "type": "model_change",
                "timestamp": "2026-04-13T09:16:32.414Z",
                "provider": "openrouter",
                "modelId": "openai/gpt-5.4"
            })),
            line(json!({
                "type": "message",
                "timestamp": "2026-04-13T09:16:37.121Z",
                "message": {
                    "role": "user",
                    "content": [{"type": "text", "text": "hi"}],
                    "timestamp": 1776071797116_u64
                }
            })),
            line(json!({
                "type": "message",
                "timestamp": "2026-04-13T09:17:13.769Z",
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": "internal"},
                        {"type": "text", "text": "hello"}
                    ],
                    "provider": "openrouter",
                    "model": "openai/gpt-5.4",
                    "usage": {"input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 1}
                }
            })),
        ];

        let session = PiParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.metadata.tool, Tool::Pi);
        assert_eq!(session.metadata.session_id, "pi-1");
        assert_eq!(session.metadata.project_path, "/proj");
        assert_eq!(
            session.metadata.model.as_deref(),
            Some("openrouter/openai/gpt-5.4")
        );
        assert_eq!(session.stats.user_messages, 1);
        assert_eq!(session.stats.assistant_messages, 1);
        assert_eq!(session.stats.total_tokens.input_tokens, 10);
        assert_eq!(session.messages[0].role, Role::System);
        assert!(matches!(
            session.messages[0].content.first().unwrap(),
            ContentBlock::ModelChange { model } if model == "openrouter/openai/gpt-5.4"
        ));
    }

    #[test]
    fn test_parse_pi_tool_call_and_result_tracks_files() {
        let lines = vec![
            line(json!({
                "type": "session",
                "version": 3,
                "id": "pi-2",
                "timestamp": "2026-04-13T09:45:42.886Z",
                "cwd": "/proj"
            })),
            line(json!({
                "type": "message",
                "timestamp": "2026-04-13T09:46:06.605Z",
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "looking"},
                        {"type": "toolCall", "id": "call_1", "name": "read", "arguments": {"path": "src/lib.rs"}}
                    ],
                    "model": "openai/gpt-5.4",
                    "usage": {"input": 100, "output": 20, "cacheRead": 0, "cacheWrite": 0}
                }
            })),
            line(json!({
                "type": "message",
                "timestamp": "2026-04-13T09:46:06.651Z",
                "message": {
                    "role": "toolResult",
                    "toolCallId": "call_1",
                    "toolName": "read",
                    "isError": false,
                    "content": [{"type": "text", "text": "file contents"}],
                    "details": {}
                }
            })),
            line(json!({
                "type": "message",
                "timestamp": "2026-04-13T09:46:07.000Z",
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "toolCall", "id": "call_2", "name": "edit", "arguments": {"path": "src/lib.rs"}}
                    ],
                    "model": "openai/gpt-5.4",
                    "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
                }
            })),
            line(json!({
                "type": "message",
                "timestamp": "2026-04-13T09:46:07.100Z",
                "message": {
                    "role": "toolResult",
                    "toolCallId": "call_2",
                    "toolName": "edit",
                    "isError": true,
                    "content": [{"type": "text", "text": "boom"}],
                    "details": {"code": 1}
                }
            })),
        ];

        let parsed = parse_lines(lines.into_iter(), "stdin".to_string(), None).unwrap();
        let session = parsed.session;
        assert_eq!(session.stats.assistant_messages, 1);
        assert_eq!(session.stats.tool_calls, 2);
        assert_eq!(session.stats.tool_errors, 1);
        assert!(session.stats.files_read.contains("src/lib.rs"));
        assert!(!session.stats.files_edited.contains("src/lib.rs"));

        assert!(session.messages.iter().any(|msg| {
            msg.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult { name, success, summary, .. }
                    if name == "read" && *success && summary == "file=\"src/lib.rs\""
                )
            })
        }));
        assert!(parsed.training.events.iter().any(|event| {
            event.event_kind == "tool_call"
                && event.tool_name.as_deref() == Some("read")
                && event.tool_call_id.as_deref() == Some("call_1")
        }));
    }

    #[test]
    fn test_format_pi_tool_input_variants() {
        assert_eq!(
            format_pi_tool_input("read", &json!({"path": "/tmp/a.rs"})),
            "file=\"/tmp/a.rs\""
        );
        assert_eq!(
            format_pi_tool_input("web_search", &json!({"query": "rust lifetimes"})),
            "query=\"rust lifetimes\""
        );
    }
}
