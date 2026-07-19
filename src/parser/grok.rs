//! Parser for Grok CLI session logs (`~/.grok/sessions/**/chat_history.jsonl`).
//!
//! Grok stores one conversation per session directory. The canonical transcript is
//! `chat_history.jsonl`, with sibling `summary.json` carrying session metadata such
//! as cwd, title, branch, and timestamps.

use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ast::*;
use crate::error::CassioError;
use crate::parser::Parser;
use crate::parser::claude::format_tool_input;
use crate::training::{
    ParsedSession, TrainingEvent, TrainingMetadata, TrainingSession, TrainingSource,
    hash_named_chunks, next_event_id, training_stats_from_session,
};

pub struct GrokParser;

#[derive(Debug, Deserialize)]
pub(crate) struct GrokSummaryFile {
    info: GrokSummaryInfo,
    created_at: String,
    #[serde(default)]
    generated_title: Option<String>,
    #[serde(default)]
    head_branch: Option<String>,
    #[serde(default)]
    current_model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GrokSummaryInfo {
    id: String,
    cwd: String,
}

impl Parser for GrokParser {
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

impl GrokParser {
    pub fn parse_from_lines<I: Iterator<Item = String>>(lines: I) -> Result<Session, CassioError> {
        Ok(parse_lines(lines, "stdin".to_string(), None)?.session)
    }
}

fn parse_lines<I: Iterator<Item = String>>(
    lines: I,
    source_path: String,
    source_root: Option<String>,
) -> Result<ParsedSession, CassioError> {
    let mut metadata = load_metadata_from_source(&source_path, source_root.as_deref());
    let mut messages: Vec<Message> = Vec::new();
    let mut stats = SessionStats::default();
    let mut current_model = metadata.as_ref().and_then(|m| m.model.clone());
    let mut models_seen: Vec<String> = Vec::new();
    let mut pending_tools: HashMap<String, (String, Value)> = HashMap::new();
    let first_timestamp = metadata.as_ref().map(|m| m.started_at);
    let last_timestamp = first_timestamp;
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

        match record_type {
            "system" => {}
            "user" => {
                let Some(text) = extract_grok_user_text(&record) else {
                    continue;
                };
                stats.user_messages += 1;
                messages.push(Message {
                    role: Role::User,
                    timestamp: None,
                    model: None,
                    content: vec![ContentBlock::Text { text: text.clone() }],
                    usage: None,
                });
                sequence += 1;
                training_events.push(TrainingEvent {
                    event_id: next_event_id(sequence),
                    sequence,
                    timestamp: None,
                    role: Some("user".to_string()),
                    event_kind: "message".to_string(),
                    model: None,
                    raw_text: Some(text),
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
            "reasoning" => {
                if let Some(summary) = reasoning_summary_text(&record) {
                    messages.push(Message {
                        role: Role::Assistant,
                        timestamp: None,
                        model: current_model.clone(),
                        content: vec![ContentBlock::Thinking { text: summary }],
                        usage: None,
                    });
                }
            }
            "assistant" => {
                let text = record.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let model_id = record
                    .get("model_id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if let Some(model) = model_id.clone() {
                    current_model = Some(model.clone());
                    if !models_seen.iter().any(|seen| seen == &model) {
                        models_seen.push(model.clone());
                    }
                    if let Some(meta) = metadata.as_mut() {
                        meta.model = Some(model);
                    }
                }

                let mut blocks = Vec::new();
                if !text.trim().is_empty() {
                    blocks.push(ContentBlock::Text {
                        text: text.to_string(),
                    });
                }

                if let Some(tool_calls) = record.get("tool_calls").and_then(|v| v.as_array()) {
                    for tool_call in tool_calls {
                        let id = tool_call
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let name = tool_call
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let input = parse_tool_arguments(tool_call.get("arguments"));
                        pending_tools.insert(id.clone(), (name.clone(), input.clone()));
                        blocks.push(ContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input,
                        });
                        stats.tool_calls += 1;
                    }
                }

                if !blocks.is_empty() {
                    if blocks
                        .iter()
                        .any(|block| matches!(block, ContentBlock::Text { .. }))
                    {
                        stats.assistant_messages += 1;
                    }
                    messages.push(Message {
                        role: Role::Assistant,
                        timestamp: None,
                        model: model_id,
                        content: blocks,
                        usage: None,
                    });
                }
            }
            "tool_result" => {
                let tool_call_id = record
                    .get("tool_call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let (name, input) = pending_tools
                    .remove(&tool_call_id)
                    .unwrap_or(("unknown".to_string(), Value::Object(Default::default())));
                let content = record.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let success = !grok_tool_result_failed(content);
                if !success {
                    stats.tool_errors += 1;
                }
                track_file_ops(&mut stats, &name, &input, !success);
                messages.push(Message {
                    role: Role::Assistant,
                    timestamp: None,
                    model: current_model.clone(),
                    content: vec![ContentBlock::ToolResult {
                        tool_use_id: tool_call_id.clone(),
                        name: name.clone(),
                        success,
                        summary: format_grok_tool_input(&name, &input),
                    }],
                    usage: None,
                });
                sequence += 1;
                training_events.push(TrainingEvent {
                    event_id: next_event_id(sequence),
                    sequence,
                    timestamp: None,
                    role: Some("assistant".to_string()),
                    event_kind: "tool_result".to_string(),
                    model: current_model.clone(),
                    raw_text: None,
                    sanitized_text: None,
                    tool_name: Some(name),
                    tool_call_id: Some(tool_call_id),
                    tool_input_raw: Some(input),
                    tool_input_sanitized: None,
                    tool_output_raw: Some(json!({ "text": content })),
                    tool_output_sanitized: None,
                    usage: None,
                    source_record_refs: vec![source_ref],
                });
            }
            _ => {}
        }
    }

    let mut meta =
        metadata.ok_or_else(|| CassioError::Other("No Grok session metadata found".into()))?;
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
        "grok.v1",
        source,
        training_metadata,
        training_stats_from_session(&session.stats),
    );
    for event in training_events {
        training.push_event(event);
    }

    Ok(ParsedSession { session, training })
}

pub(crate) fn load_grok_summary(path: &Path) -> Option<GrokSummaryFile> {
    let summary_path = path.parent()?.join("summary.json");
    let content = std::fs::read_to_string(summary_path).ok()?;
    serde_json::from_str(&content).ok()
}

pub(crate) fn grok_project_path_from_source(path: &Path) -> Option<String> {
    if let Some(summary) = load_grok_summary(path) {
        return Some(summary.info.cwd);
    }

    let path_str = path.to_string_lossy();
    let marker = ".grok/sessions/";
    let after = path_str.split(marker).nth(1)?;
    let encoded = after.split('/').next()?;
    Some(percent_decode(encoded))
}

pub(crate) fn grok_session_id_from_source(path: &Path) -> Option<String> {
    if let Some(summary) = load_grok_summary(path) {
        return Some(summary.info.id);
    }
    let parent = path.parent()?;
    parent
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
}

pub(crate) fn grok_started_at_from_source(path: &Path) -> Option<DateTime<Utc>> {
    if let Some(summary) = load_grok_summary(path) {
        return summary.created_at.parse::<DateTime<Utc>>().ok();
    }
    grok_session_id_from_source(path)
        .as_deref()
        .and_then(uuid_v7_timestamp)
}

fn load_metadata_from_source(
    source_path: &str,
    source_root: Option<&str>,
) -> Option<SessionMetadata> {
    if source_path == "stdin" {
        return Some(SessionMetadata {
            session_id: "stdin".to_string(),
            tool: Tool::Grok,
            project_path: source_root.unwrap_or("").to_string(),
            started_at: Utc::now(),
            session_kind: SessionKind::Uncertain,
            version: None,
            git_branch: None,
            model: None,
            title: None,
        });
    }

    let path = Path::new(source_path);
    let summary = load_grok_summary(path);
    let session_id = summary
        .as_ref()
        .map(|s| s.info.id.clone())
        .or_else(|| grok_session_id_from_source(path))
        .unwrap_or_else(|| "unknown".to_string());
    let project_path = summary
        .as_ref()
        .map(|s| s.info.cwd.clone())
        .or_else(|| grok_project_path_from_source(path))
        .unwrap_or_default();
    let started_at = summary
        .as_ref()
        .and_then(|s| s.created_at.parse::<DateTime<Utc>>().ok())
        .or_else(|| grok_started_at_from_source(path))
        .unwrap_or_else(Utc::now);

    Some(SessionMetadata {
        session_id,
        tool: Tool::Grok,
        project_path,
        started_at,
        session_kind: SessionKind::Uncertain,
        version: None,
        git_branch: summary.as_ref().and_then(|s| s.head_branch.clone()),
        model: summary.as_ref().and_then(|s| s.current_model_id.clone()),
        title: summary.as_ref().and_then(|s| s.generated_title.clone()),
    })
}

fn extract_grok_user_text(record: &Value) -> Option<String> {
    let content = record.get("content")?;
    let text = if let Some(text) = content.as_str() {
        text.to_string()
    } else if let Some(blocks) = content.as_array() {
        blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        return None;
    };

    if let Some(query) = extract_tag_content(&text, "user_query") {
        return Some(query);
    }

    let trimmed = text.trim();
    if trimmed.starts_with("<user_info>")
        || trimmed.starts_with("<git_status>")
        || trimmed.starts_with("<rules>")
        || trimmed.starts_with("<agent_skills>")
    {
        return None;
    }

    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn extract_tag_content(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    let content = text[start..end].trim();
    if content.is_empty() {
        None
    } else {
        Some(content.to_string())
    }
}

fn reasoning_summary_text(record: &Value) -> Option<String> {
    record
        .get("summary")
        .and_then(|v| v.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.trim().is_empty())
}

fn parse_tool_arguments(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Object(map)) => Value::Object(map.clone()),
        Some(Value::String(raw)) => {
            serde_json::from_str(raw).unwrap_or_else(|_| json!({ "raw": raw }))
        }
        Some(other) => other.clone(),
        None => Value::Object(Default::default()),
    }
}

fn grok_tool_result_failed(content: &str) -> bool {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Exit code:") {
            let code = rest.trim();
            if code != "0" && !code.is_empty() {
                return true;
            }
        }
        if let Some(rest) = trimmed.strip_prefix("last_exit_code:") {
            let code = rest.trim();
            if code != "0" && !code.is_empty() {
                return true;
            }
        }
    }
    false
}

fn format_grok_tool_input(tool_name: &str, input: &Value) -> String {
    if let Some(path) = input.get("path").and_then(|v| v.as_str()) {
        match tool_name {
            "Read" | "read" | "Write" | "write" | "Edit" | "edit" | "StrReplace"
            | "search_replace" => {
                return format!("file=\"{path}\"");
            }
            "Glob" | "glob" => {
                return format!("pattern=\"{path}\"");
            }
            _ => {}
        }
    }
    format_tool_input(tool_name, input)
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
        "Read" | "read" => {
            stats.files_read.insert(path.to_string());
        }
        "Write" | "write" => {
            stats.files_written.insert(path.to_string());
        }
        "Edit" | "edit" | "StrReplace" | "search_replace" => {
            stats.files_edited.insert(path.to_string());
        }
        _ => {}
    }
}

fn percent_decode(segment: &str) -> String {
    let bytes = segment.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&segment[index + 1..index + 3], 16)
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn uuid_v7_timestamp(id: &str) -> Option<DateTime<Utc>> {
    let hex = id.replace('-', "");
    if hex.len() < 12 {
        return None;
    }
    let millis = u64::from_str_radix(&hex[..12], 16).ok()?;
    DateTime::from_timestamp_millis(millis as i64)
}

#[cfg(test)]
#[path = "grok_test.rs"]
mod tests;
