//! Parser for Cursor agent transcripts (`~/.cursor/projects/**/agent-transcripts/**/*.jsonl`).

use std::io::BufRead;
use std::path::Path;
use std::time::UNIX_EPOCH;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::ast::*;
use crate::error::CassioError;
use crate::parser::Parser;
use crate::training::{
    ParsedSession, TrainingEvent, TrainingMetadata, TrainingSession, TrainingSource,
    hash_named_chunks, next_event_id, training_stats_from_session,
};

pub struct CursorParser;

impl Parser for CursorParser {
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

impl CursorParser {
    pub fn parse_from_lines<I: Iterator<Item = String>>(lines: I) -> Result<Session, CassioError> {
        Ok(parse_lines(lines, "stdin".to_string(), None)?.session)
    }
}

fn parse_lines<I: Iterator<Item = String>>(
    lines: I,
    source_path: String,
    source_root: Option<String>,
) -> Result<ParsedSession, CassioError> {
    let path = Path::new(&source_path);
    let metadata = load_metadata_from_source(path, source_root.as_deref());
    let mut messages: Vec<Message> = Vec::new();
    let mut stats = SessionStats::default();
    let first_timestamp = metadata.as_ref().map(|m| m.started_at);
    let last_timestamp = first_timestamp;
    let mut training_events: Vec<TrainingEvent> = Vec::new();
    let mut sequence: u64 = 0;
    let mut line_count: u64 = 0;
    let mut hash_chunks: Vec<(String, String)> = Vec::new();
    let mut tool_counter: u64 = 0;

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
        let role = record.get("role").and_then(|v| v.as_str()).unwrap_or("");
        let message = record.get("message");

        match role {
            "user" => {
                let Some(text) = extract_cursor_user_text(message) else {
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
            "assistant" => {
                let mut blocks = Vec::new();
                let mut has_text = false;
                if let Some(content) = message
                    .and_then(|m| m.get("content"))
                    .and_then(|v| v.as_array())
                {
                    for block in content {
                        match block.get("type").and_then(|v| v.as_str()) {
                            Some("text") => {
                                let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                if !text.trim().is_empty() {
                                    has_text = true;
                                    blocks.push(ContentBlock::Text {
                                        text: text.to_string(),
                                    });
                                }
                            }
                            Some("tool_use") => {
                                tool_counter += 1;
                                let name = block
                                    .get("name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown")
                                    .to_string();
                                let input = block
                                    .get("input")
                                    .cloned()
                                    .unwrap_or_else(|| Value::Object(Default::default()));
                                let id = format!("cursor-tool-{tool_counter}");
                                blocks.push(ContentBlock::ToolUse {
                                    id: id.clone(),
                                    name: name.clone(),
                                    input: input.clone(),
                                });
                                stats.tool_calls += 1;
                                track_file_ops(&mut stats, &name, &input);
                                sequence += 1;
                                training_events.push(TrainingEvent {
                                    event_id: next_event_id(sequence),
                                    sequence,
                                    timestamp: None,
                                    role: Some("assistant".to_string()),
                                    event_kind: "tool_call".to_string(),
                                    model: None,
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
                        timestamp: None,
                        model: None,
                        content: blocks,
                        usage: None,
                    });
                }
            }
            _ => {}
        }
    }

    let mut meta =
        metadata.ok_or_else(|| CassioError::Other("No Cursor session metadata found".into()))?;
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
        models_seen: Vec::new(),
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
        "cursor.v1",
        source,
        training_metadata,
        training_stats_from_session(&session.stats),
    );
    for event in training_events {
        training.push_event(event);
    }

    Ok(ParsedSession { session, training })
}

pub(crate) fn cursor_project_path_from_source(path: &Path) -> Option<String> {
    let path_str = path.to_string_lossy();
    let marker = ".cursor/projects/";
    let after = path_str.split(marker).nth(1)?;
    let slug = after.split('/').next()?;
    if slug.is_empty() {
        return None;
    }
    Some(format!("/{}", slug.replace('-', "/")))
}

pub(crate) fn cursor_started_at_from_source(path: &Path) -> Option<DateTime<Utc>> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    DateTime::from_timestamp(duration.as_secs() as i64, duration.subsec_nanos())
}

fn load_metadata_from_source(path: &Path, source_root: Option<&str>) -> Option<SessionMetadata> {
    if path.to_string_lossy() == "stdin" {
        return Some(SessionMetadata {
            session_id: "stdin".to_string(),
            tool: Tool::Cursor,
            project_path: source_root.unwrap_or("").to_string(),
            started_at: Utc::now(),
            session_kind: SessionKind::Uncertain,
            version: None,
            git_branch: None,
            model: None,
            title: None,
        });
    }

    let session_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("unknown")
        .to_string();
    let project_path = cursor_project_path_from_source(path).unwrap_or_default();
    let started_at = cursor_started_at_from_source(path).unwrap_or_else(Utc::now);

    Some(SessionMetadata {
        session_id,
        tool: Tool::Cursor,
        project_path,
        started_at,
        session_kind: SessionKind::Uncertain,
        version: None,
        git_branch: None,
        model: None,
        title: None,
    })
}

fn extract_cursor_user_text(message: Option<&Value>) -> Option<String> {
    let content = message?.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn track_file_ops(stats: &mut SessionStats, tool_name: &str, input: &Value) {
    let path = input
        .get("path")
        .and_then(|v| v.as_str())
        .or_else(|| input.get("target_notebook").and_then(|v| v.as_str()));
    let Some(path) = path else {
        return;
    };

    match tool_name {
        "Read" => {
            stats.files_read.insert(path.to_string());
        }
        "Write" => {
            stats.files_written.insert(path.to_string());
        }
        "StrReplace" | "Edit" | "search_replace" => {
            stats.files_edited.insert(path.to_string());
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "cursor_test.rs"]
mod tests;
