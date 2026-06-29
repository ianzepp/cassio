//! Parser for Hermes session stores.
//!
//! Hermes has had two storage layouts:
//! - Current: one SQLite database at `~/.hermes/state.db` with `sessions` and
//!   `messages` tables.
//! - Legacy: per-session JSON snapshots and occasional JSONL files under
//!   `~/.hermes/sessions`.
//!
//! Discovery passes SQLite rows as virtual paths of the form `state.db/<session_id>`.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Map, Value, json};

use crate::ast::{
    ContentBlock, Message, Role, Session, SessionKind, SessionMetadata, SessionStats, TokenUsage,
    Tool, classify_session_kind,
};
use crate::error::CassioError;
use crate::parser::{Parser, truncate};
use crate::training::{
    ParsedSession, TrainingEvent, TrainingMetadata, TrainingSession, TrainingSource,
    hash_named_chunks, next_event_id, training_stats_from_session,
};

const CONTENT_JSON_PREFIX: &str = "\0json:";

pub struct HermesParser;

impl Parser for HermesParser {
    fn parse_export(&self, path: &Path) -> Result<ParsedSession, CassioError> {
        if let Some((db_path, session_id)) = split_state_db_virtual_path(path) {
            return parse_state_db_session(&db_path, &session_id, path);
        }

        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => parse_legacy_json(path),
            Some("jsonl") => parse_legacy_jsonl(path),
            _ => Err(CassioError::UnknownFormat(path.to_path_buf())),
        }
    }
}

fn split_state_db_virtual_path(path: &Path) -> Option<(PathBuf, String)> {
    let path_str = path.to_string_lossy();
    let (db, session_id) = path_str.split_once("state.db/")?;
    Some((
        PathBuf::from(format!("{db}state.db")),
        session_id.to_string(),
    ))
}

#[derive(Debug)]
struct RawHermesSession {
    source_path: String,
    source_format: String,
    source_root: Option<String>,
    source_hash: String,
    source_record_count: Option<u64>,
    session_id: String,
    source: String,
    model: Option<String>,
    title: Option<String>,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    messages: Vec<RawHermesMessage>,
    total_tokens: TokenUsage,
    cost: Option<f64>,
}

#[derive(Debug)]
struct RawHermesMessage {
    role: String,
    timestamp: Option<DateTime<Utc>>,
    content: Option<Value>,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    tool_calls: Option<Value>,
    reasoning: Option<String>,
    reasoning_content: Option<String>,
    finish_reason: Option<String>,
}

fn parse_state_db_session(
    db_path: &Path,
    session_id: &str,
    virtual_path: &Path,
) -> Result<ParsedSession, CassioError> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| CassioError::Other(format!("Failed to open Hermes state DB: {e}")))?;

    let session_row = conn
        .query_row(
            "SELECT id, source, model, title, started_at, ended_at, input_tokens, output_tokens, \
                    cache_read_tokens, cache_write_tokens, estimated_cost_usd, actual_cost_usd \
             FROM sessions WHERE id = ?",
            [session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, Option<u64>>(6)?,
                    row.get::<_, Option<u64>>(7)?,
                    row.get::<_, Option<u64>>(8)?,
                    row.get::<_, Option<u64>>(9)?,
                    row.get::<_, Option<f64>>(10)?,
                    row.get::<_, Option<f64>>(11)?,
                ))
            },
        )
        .map_err(|e| {
            CassioError::Other(format!("Failed to read Hermes session {session_id}: {e}"))
        })?;

    let mut stmt = conn
        .prepare(
            "SELECT role, content, tool_call_id, tool_calls, tool_name, timestamp, finish_reason, \
                    reasoning, reasoning_content \
             FROM messages WHERE session_id = ? ORDER BY timestamp, id",
        )
        .map_err(|e| CassioError::Other(format!("Failed to read Hermes messages: {e}")))?;

    let rows = stmt
        .query_map([session_id], |row| {
            let content: Option<String> = row.get(1)?;
            Ok(RawHermesMessage {
                role: row.get(0)?,
                timestamp: epoch_to_utc(row.get::<_, f64>(5)?),
                content: content.map(decode_content_value),
                tool_call_id: row.get(2)?,
                tool_calls: row
                    .get::<_, Option<String>>(3)?
                    .and_then(|s| serde_json::from_str(&s).ok()),
                tool_name: row.get(4)?,
                finish_reason: row.get(6)?,
                reasoning: row.get(7)?,
                reasoning_content: row.get(8)?,
            })
        })
        .map_err(|e| CassioError::Other(format!("Failed to read Hermes messages: {e}")))?;

    let messages = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| CassioError::Other(format!("Failed to decode Hermes messages: {e}")))?;

    let source_hash = hash_named_chunks([(
        virtual_path.to_string_lossy().to_string(),
        fs::metadata(db_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos().to_string())
            .unwrap_or_else(|| session_id.to_string()),
    )]);

    let raw = RawHermesSession {
        source_path: virtual_path.to_string_lossy().to_string(),
        source_format: "hermes.sqlite".to_string(),
        source_root: db_path.parent().map(|p| p.to_string_lossy().to_string()),
        source_hash,
        source_record_count: Some(messages.len() as u64),
        session_id: session_row.0,
        source: session_row.1,
        model: session_row.2,
        title: session_row.3,
        started_at: epoch_to_utc(session_row.4).unwrap_or_else(Utc::now),
        ended_at: session_row.5.and_then(epoch_to_utc),
        messages,
        total_tokens: TokenUsage {
            input_tokens: session_row.6.unwrap_or(0),
            output_tokens: session_row.7.unwrap_or(0),
            cache_read_tokens: session_row.8.unwrap_or(0),
            cache_creation_tokens: session_row.9.unwrap_or(0),
        },
        cost: session_row.11.or(session_row.10),
    };

    normalize(raw)
}

fn parse_legacy_json(path: &Path) -> Result<ParsedSession, CassioError> {
    let content = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&content).map_err(|source| CassioError::Json {
        path: path.to_path_buf(),
        source,
    })?;

    let messages = value
        .get("messages")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(raw_message_from_value)
        .collect();

    let raw = RawHermesSession {
        source_path: path.to_string_lossy().to_string(),
        source_format: "hermes.session-json".to_string(),
        source_root: path.parent().map(|p| p.to_string_lossy().to_string()),
        source_hash: hash_named_chunks([(path.to_string_lossy().to_string(), content)]),
        source_record_count: value
            .get("message_count")
            .and_then(|n| n.as_u64())
            .or_else(|| {
                value
                    .get("messages")
                    .and_then(|m| m.as_array())
                    .map(|m| m.len() as u64)
            }),
        session_id: value
            .get("session_id")
            .and_then(|s| s.as_str())
            .map(str::to_string)
            .or_else(|| session_id_from_filename(path))
            .unwrap_or_else(|| "unknown".to_string()),
        source: value
            .get("platform")
            .and_then(|s| s.as_str())
            .unwrap_or("hermes")
            .to_string(),
        model: value
            .get("model")
            .and_then(|s| s.as_str())
            .map(str::to_string),
        title: value
            .get("title")
            .and_then(|s| s.as_str())
            .map(str::to_string),
        started_at: value
            .get("session_start")
            .and_then(|s| s.as_str())
            .and_then(parse_naive_or_rfc3339)
            .unwrap_or_else(Utc::now),
        ended_at: value
            .get("last_updated")
            .and_then(|s| s.as_str())
            .and_then(parse_naive_or_rfc3339),
        messages,
        total_tokens: TokenUsage::default(),
        cost: None,
    };

    normalize(raw)
}

fn parse_legacy_jsonl(path: &Path) -> Result<ParsedSession, CassioError> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut raw_lines = Vec::new();
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line).map_err(|source| CassioError::Json {
            path: path.to_path_buf(),
            source,
        })?;
        raw_lines.push(line);
        records.push(value);
    }

    let mut model = None;
    let mut source = "hermes".to_string();
    let mut messages = Vec::new();
    for record in records {
        if model.is_none() {
            model = record
                .get("model")
                .and_then(|s| s.as_str())
                .map(str::to_string);
        }
        if let Some(platform) = record.get("platform").and_then(|s| s.as_str()) {
            source = platform.to_string();
        }
        if record.get("content").is_some() || record.get("tool_calls").is_some() {
            messages.push(raw_message_from_value(record));
        }
    }

    let started_at = messages
        .iter()
        .find_map(|m| m.timestamp)
        .or_else(|| session_id_from_filename(path).and_then(|id| parse_session_id_time(&id)))
        .unwrap_or_else(Utc::now);
    let ended_at = messages.iter().rev().find_map(|m| m.timestamp);

    let raw = RawHermesSession {
        source_path: path.to_string_lossy().to_string(),
        source_format: "hermes.session-jsonl".to_string(),
        source_root: path.parent().map(|p| p.to_string_lossy().to_string()),
        source_hash: hash_named_chunks(
            raw_lines
                .into_iter()
                .enumerate()
                .map(|(i, line)| (format!("line-{i}"), line)),
        ),
        source_record_count: Some(messages.len() as u64),
        session_id: session_id_from_filename(path).unwrap_or_else(|| "unknown".to_string()),
        source,
        model,
        title: None,
        started_at,
        ended_at,
        messages,
        total_tokens: TokenUsage::default(),
        cost: None,
    };

    normalize(raw)
}

fn raw_message_from_value(value: Value) -> RawHermesMessage {
    RawHermesMessage {
        role: value
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("system")
            .to_string(),
        timestamp: value.get("timestamp").and_then(|t| {
            t.as_f64()
                .and_then(epoch_to_utc)
                .or_else(|| t.as_str().and_then(parse_naive_or_rfc3339))
        }),
        content: value.get("content").cloned(),
        tool_call_id: value
            .get("tool_call_id")
            .and_then(|s| s.as_str())
            .map(str::to_string),
        tool_name: value
            .get("name")
            .or_else(|| value.get("tool_name"))
            .and_then(|s| s.as_str())
            .map(str::to_string),
        tool_calls: value.get("tool_calls").cloned(),
        reasoning: value
            .get("reasoning")
            .and_then(|s| s.as_str())
            .map(str::to_string),
        reasoning_content: value
            .get("reasoning_content")
            .and_then(|s| s.as_str())
            .map(str::to_string),
        finish_reason: value
            .get("finish_reason")
            .and_then(|s| s.as_str())
            .map(str::to_string),
    }
}

fn normalize(raw: RawHermesSession) -> Result<ParsedSession, CassioError> {
    let mut messages = Vec::new();
    let mut stats = SessionStats {
        total_tokens: raw.total_tokens.clone(),
        cost: raw.cost,
        ..Default::default()
    };
    let mut training_events = Vec::new();
    let mut models_seen = BTreeSet::new();
    if let Some(model) = raw.model.as_ref() {
        models_seen.insert(model.clone());
    }
    let mut pending_tools: HashMap<String, String> = HashMap::new();
    let mut sequence = 0u64;

    for (record_index, raw_msg) in raw.messages.iter().enumerate() {
        let role = match raw_msg.role.as_str() {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" => Role::System,
            "system" | "session_meta" => Role::System,
            _ => Role::System,
        };

        let mut blocks = Vec::new();
        if role == Role::Assistant
            && let Some(reasoning) = raw_msg
                .reasoning_content
                .as_deref()
                .or(raw_msg.reasoning.as_deref())
                .filter(|s| !s.trim().is_empty())
        {
            blocks.push(ContentBlock::Thinking {
                text: reasoning.to_string(),
            });
        }

        if let Some(text) = raw_msg.content.as_ref().and_then(content_text)
            && !text.trim().is_empty()
            && raw_msg.role != "tool"
        {
            blocks.push(ContentBlock::Text { text: text.clone() });
            training_events.push(text_event(
                &mut sequence,
                raw_msg.timestamp,
                role,
                raw.model.clone(),
                text,
                record_index,
            ));
        }

        if raw_msg.role == "assistant"
            && let Some(calls) = raw_msg.tool_calls.as_ref().and_then(|v| v.as_array())
        {
            for call in calls {
                let (id, name, input) = parse_tool_call(call);
                pending_tools.insert(id.clone(), name.clone());
                stats.tool_calls += 1;
                track_tool_input(&name, &input, &mut stats);
                blocks.push(ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
                training_events.push(tool_use_event(
                    &mut sequence,
                    raw_msg.timestamp,
                    raw.model.clone(),
                    &id,
                    &name,
                    input,
                    record_index,
                ));
            }
        }

        if raw_msg.role == "tool" {
            let id = raw_msg
                .tool_call_id
                .clone()
                .unwrap_or_else(|| format!("tool-result-{record_index}"));
            let name = raw_msg
                .tool_name
                .clone()
                .or_else(|| pending_tools.get(&id).cloned())
                .unwrap_or_else(|| "tool".to_string());
            let output = raw_msg
                .content
                .as_ref()
                .and_then(content_text)
                .unwrap_or_default();
            let success = !looks_like_error(&output, raw_msg.finish_reason.as_deref());
            if !success {
                stats.tool_errors += 1;
            }
            blocks.push(ContentBlock::ToolResult {
                tool_use_id: id.clone(),
                name: name.clone(),
                success,
                summary: truncate(&output, 500).to_string(),
            });
            training_events.push(tool_result_event(
                &mut sequence,
                raw_msg.timestamp,
                &id,
                &name,
                output,
                record_index,
            ));
        }

        if blocks.is_empty() {
            continue;
        }

        match role {
            Role::User => stats.user_messages += 1,
            Role::Assistant => stats.assistant_messages += 1,
            Role::System => {}
        }

        messages.push(Message {
            role,
            timestamp: raw_msg.timestamp,
            model: if role == Role::Assistant {
                raw.model.clone()
            } else {
                None
            },
            content: blocks,
            usage: None,
        });
    }

    stats.duration_seconds = raw
        .ended_at
        .map(|ended| (ended - raw.started_at).num_seconds())
        .or_else(|| {
            let first = messages.iter().find_map(|m| m.timestamp)?;
            let last = messages.iter().rev().find_map(|m| m.timestamp)?;
            Some((last - first).num_seconds())
        });

    let session = Session {
        metadata: SessionMetadata {
            session_id: raw.session_id.clone(),
            tool: Tool::Hermes,
            project_path: format!("hermes:{}", raw.source),
            started_at: raw.started_at,
            session_kind: SessionKind::Uncertain,
            version: None,
            git_branch: None,
            model: raw.model.clone(),
            title: raw.title.clone(),
        },
        messages,
        stats,
    };

    let mut session = session;
    session.metadata.session_kind = classify_session_kind(&session.messages);

    let mut training = TrainingSession::new(
        "hermes.v1",
        TrainingSource {
            tool: "hermes".to_string(),
            source_path: raw.source_path,
            session_id: raw.session_id,
            source_hash: raw.source_hash,
            source_record_count: raw.source_record_count,
            source_format: Some(raw.source_format),
            source_root: raw.source_root,
        },
        TrainingMetadata {
            project_path_raw: session.metadata.project_path.clone(),
            project_path_sanitized: session.metadata.project_path.clone(),
            started_at: session.metadata.started_at,
            ended_at: raw.ended_at,
            git_branch: None,
            title: session.metadata.title.clone(),
            session_kind: session.metadata.session_kind.to_string(),
            models_seen: models_seen.into_iter().collect(),
            version: None,
        },
        training_stats_from_session(&session.stats),
    );
    for event in training_events {
        training.push_event(event);
    }

    Ok(ParsedSession { session, training })
}

fn content_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for part in parts {
                if let Some(text) = part
                    .get("text")
                    .or_else(|| part.get("content"))
                    .and_then(|v| v.as_str())
                {
                    out.push(text.to_string());
                }
            }
            Some(out.join("\n"))
        }
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn decode_content_value(content: String) -> Value {
    if let Some(json_content) = content.strip_prefix(CONTENT_JSON_PREFIX)
        && let Ok(value) = serde_json::from_str(json_content)
    {
        return value;
    }
    Value::String(content)
}

fn parse_tool_call(call: &Value) -> (String, String, Value) {
    let id = call
        .get("id")
        .or_else(|| call.get("call_id"))
        .and_then(|s| s.as_str())
        .unwrap_or("tool-call")
        .to_string();

    let function = call.get("function").and_then(|f| f.as_object());
    let name = function
        .and_then(|f| f.get("name"))
        .or_else(|| call.get("name"))
        .and_then(|s| s.as_str())
        .unwrap_or("tool")
        .to_string();

    let raw_args = function
        .and_then(|f| f.get("arguments"))
        .or_else(|| call.get("arguments"));
    let input = match raw_args {
        Some(Value::String(s)) => {
            serde_json::from_str(s).unwrap_or_else(|_| json!({ "arguments": s }))
        }
        Some(v) => v.clone(),
        None => Value::Object(Map::new()),
    };

    (id, name, input)
}

fn track_tool_input(name: &str, input: &Value, stats: &mut SessionStats) {
    if let Some(path) = first_string_field(input, &["path", "file", "file_path", "filename"]) {
        match name {
            "read_file" | "read" | "view" => {
                stats.files_read.insert(path);
            }
            "edit" | "replace" | "write_file" | "write" => {
                stats.files_written.insert(path);
            }
            _ => {}
        }
    }
}

fn first_string_field(value: &Value, keys: &[&str]) -> Option<String> {
    let obj = value.as_object()?;
    keys.iter()
        .find_map(|key| obj.get(*key).and_then(|v| v.as_str()))
        .map(str::to_string)
}

fn looks_like_error(output: &str, finish_reason: Option<&str>) -> bool {
    if matches!(finish_reason, Some("error")) {
        return true;
    }
    let lower = output.trim_start().to_lowercase();
    lower.starts_with("error:") || lower.starts_with("traceback ")
}

fn text_event(
    sequence: &mut u64,
    timestamp: Option<DateTime<Utc>>,
    role: Role,
    model: Option<String>,
    text: String,
    record_index: usize,
) -> TrainingEvent {
    *sequence += 1;
    TrainingEvent {
        event_id: next_event_id(*sequence),
        sequence: *sequence,
        timestamp,
        role: Some(role_name(role).to_string()),
        event_kind: "message".to_string(),
        model,
        raw_text: Some(text.clone()),
        sanitized_text: Some(text),
        tool_name: None,
        tool_call_id: None,
        tool_input_raw: None,
        tool_input_sanitized: None,
        tool_output_raw: None,
        tool_output_sanitized: None,
        usage: None,
        source_record_refs: vec![format!("record:{record_index}")],
    }
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    }
}

fn tool_use_event(
    sequence: &mut u64,
    timestamp: Option<DateTime<Utc>>,
    model: Option<String>,
    id: &str,
    name: &str,
    input: Value,
    record_index: usize,
) -> TrainingEvent {
    *sequence += 1;
    TrainingEvent {
        event_id: next_event_id(*sequence),
        sequence: *sequence,
        timestamp,
        role: Some("assistant".to_string()),
        event_kind: "tool_use".to_string(),
        model,
        raw_text: None,
        sanitized_text: None,
        tool_name: Some(name.to_string()),
        tool_call_id: Some(id.to_string()),
        tool_input_raw: Some(input.clone()),
        tool_input_sanitized: Some(input),
        tool_output_raw: None,
        tool_output_sanitized: None,
        usage: None,
        source_record_refs: vec![format!("record:{record_index}")],
    }
}

fn tool_result_event(
    sequence: &mut u64,
    timestamp: Option<DateTime<Utc>>,
    id: &str,
    name: &str,
    output: String,
    record_index: usize,
) -> TrainingEvent {
    *sequence += 1;
    let output_value = Value::String(output);
    TrainingEvent {
        event_id: next_event_id(*sequence),
        sequence: *sequence,
        timestamp,
        role: Some("system".to_string()),
        event_kind: "tool_result".to_string(),
        model: None,
        raw_text: None,
        sanitized_text: None,
        tool_name: Some(name.to_string()),
        tool_call_id: Some(id.to_string()),
        tool_input_raw: None,
        tool_input_sanitized: None,
        tool_output_raw: Some(output_value.clone()),
        tool_output_sanitized: Some(output_value),
        usage: None,
        source_record_refs: vec![format!("record:{record_index}")],
    }
}

fn epoch_to_utc(epoch: f64) -> Option<DateTime<Utc>> {
    let secs = epoch.trunc() as i64;
    let nanos = ((epoch.fract() * 1_000_000_000.0).round() as u32).min(999_999_999);
    Utc.timestamp_opt(secs, nanos).single()
}

fn parse_naive_or_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|dt| Utc.from_utc_datetime(&dt))
        })
}

fn parse_session_id_time(id: &str) -> Option<DateTime<Utc>> {
    if id.len() < 15 {
        return None;
    }
    let ts = format!(
        "{}-{}-{}T{}:{}:{}",
        &id[0..4],
        &id[4..6],
        &id[6..8],
        &id[9..11],
        &id[11..13],
        &id[13..15]
    );
    parse_naive_or_rfc3339(&ts)
}

fn session_id_from_filename(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_prefix("session_")
        .and_then(|s| s.strip_suffix(".json"))
        .or_else(|| name.strip_suffix(".jsonl"))
        .map(str::to_string)
}

#[cfg(test)]
#[path = "hermes_test.rs"]
mod tests;
