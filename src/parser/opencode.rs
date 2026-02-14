use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::ast::*;
use crate::error::CassioError;
use crate::parser::Parser;

pub struct OpenCodeParser;

impl Parser for OpenCodeParser {
    fn parse_session(&self, path: &Path) -> Result<Session, CassioError> {
        // For OpenCode, path should be the storage directory and we need a session ID.
        // If path is a directory containing session/message/part subdirs, treat it as storage root.
        // If path points to a specific session directory under message/, derive session ID.

        // Heuristic: if path contains "message/<ses_...>", it's a message dir
        let path_str = path.to_string_lossy();

        if path_str.contains("/message/ses_") {
            // Extract session ID from path
            let session_id = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let storage_dir = path.parent().and_then(|p| p.parent()).ok_or_else(|| {
                CassioError::Other("Cannot determine storage directory".into())
            })?;
            return parse_session(storage_dir, &session_id);
        }

        // If it's a storage directory, enumerate sessions
        let message_dir = path.join("message");
        if message_dir.is_dir() {
            // Find first session
            let entries = std::fs::read_dir(&message_dir)?;
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("ses_") {
                    return parse_session(path, &name);
                }
            }
        }

        Err(CassioError::Other(format!(
            "Cannot find OpenCode session data at: {}",
            path.display()
        )))
    }
}

#[derive(Deserialize)]
struct OCSession {
    id: String,
    directory: Option<String>,
    title: Option<String>,
    time: Option<OCTime>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct OCTime {
    created: Option<f64>,
    updated: Option<f64>,
}

#[derive(Deserialize)]
struct OCMessage {
    id: String,
    role: Option<String>,
    time: Option<OCMsgTime>,
    #[serde(rename = "modelID")]
    model_id: Option<String>,
    cost: Option<f64>,
    tokens: Option<OCTokens>,
}

#[derive(Deserialize)]
struct OCMsgTime {
    created: Option<f64>,
    completed: Option<f64>,
}

#[derive(Deserialize)]
struct OCTokens {
    input: Option<u64>,
    output: Option<u64>,
    cache: Option<OCCache>,
}

#[derive(Deserialize)]
struct OCCache {
    read: Option<u64>,
    write: Option<u64>,
}

#[derive(Deserialize)]
struct OCPart {
    #[serde(rename = "type")]
    part_type: Option<String>,
    text: Option<String>,
    synthetic: Option<bool>,
    tool: Option<String>,
    state: Option<OCPartState>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct OCPartState {
    status: Option<String>,
    input: Option<Value>,
    title: Option<String>,
    metadata: Option<OCPartMeta>,
}

#[derive(Deserialize)]
struct OCPartMeta {
    exit: Option<i32>,
    description: Option<String>,
}

fn parse_session(storage_dir: &Path, session_id: &str) -> Result<Session, CassioError> {
    // Load session file
    let session_data = find_session_file(storage_dir, session_id)?;

    // Load messages
    let messages_dir = storage_dir.join("message").join(session_id);
    let mut oc_messages = load_messages(&messages_dir)?;
    oc_messages.sort_by(|a, b| {
        let ta = a.time.as_ref().and_then(|t| t.created).unwrap_or(0.0);
        let tb = b.time.as_ref().and_then(|t| t.created).unwrap_or(0.0);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });

    // Load parts per message
    let mut parts_map: HashMap<String, Vec<OCPart>> = HashMap::new();
    for msg in &oc_messages {
        let parts_dir = storage_dir.join("part").join(&msg.id);
        if parts_dir.is_dir() {
            let parts = load_parts(&parts_dir)?;
            if !parts.is_empty() {
                parts_map.insert(msg.id.clone(), parts);
            }
        }
    }

    // Build AST
    let started_at = session_data
        .time
        .as_ref()
        .and_then(|t| t.created)
        .map(|ts| timestamp_from_millis(ts as i64))
        .unwrap_or_else(Utc::now);

    let mut metadata = SessionMetadata {
        session_id: session_data.id.clone(),
        tool: Tool::OpenCode,
        project_path: session_data.directory.unwrap_or_default(),
        started_at,
        version: None,
        git_branch: None,
        model: None,
        title: session_data.title,
    };

    let mut stats = SessionStats::default();
    let mut messages: Vec<Message> = Vec::new();
    let mut current_model: Option<String> = None;
    let mut total_cost: f64 = 0.0;
    let mut last_timestamp: Option<DateTime<Utc>> = None;

    for oc_msg in &oc_messages {
        let msg_ts = oc_msg
            .time
            .as_ref()
            .and_then(|t| t.completed.or(t.created))
            .map(|ts| timestamp_from_millis(ts as i64));

        if let Some(t) = msg_ts {
            last_timestamp = Some(t);
        }

        // Track tokens
        if let Some(ref tokens) = oc_msg.tokens {
            stats.total_tokens.input_tokens += tokens.input.unwrap_or(0);
            stats.total_tokens.output_tokens += tokens.output.unwrap_or(0);
            if let Some(ref cache) = tokens.cache {
                stats.total_tokens.cache_read_tokens += cache.read.unwrap_or(0);
                stats.total_tokens.cache_creation_tokens += cache.write.unwrap_or(0);
            }
        }

        if let Some(cost) = oc_msg.cost {
            total_cost += cost;
        }

        // Model change
        if let Some(ref model) = oc_msg.model_id {
            if current_model.as_ref() != Some(model) {
                current_model = Some(model.clone());
                messages.push(Message {
                    role: Role::System,
                    timestamp: msg_ts,
                    model: Some(model.clone()),
                    content: vec![ContentBlock::ModelChange {
                        model: model.clone(),
                    }],
                    usage: None,
                });
            }
        }

        let msg_parts = parts_map.remove(&oc_msg.id).unwrap_or_default();
        let role_str = oc_msg.role.as_deref().unwrap_or("");

        if role_str == "user" {
            let mut blocks = Vec::new();
            let mut has_text = false;

            for part in &msg_parts {
                let pt = part.part_type.as_deref().unwrap_or("");
                if pt == "text" {
                    if part.synthetic.unwrap_or(false) {
                        continue;
                    }
                    if let Some(text) = &part.text {
                        if text.starts_with("<file>") || text.starts_with("Called the") {
                            continue;
                        }
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            blocks.push(ContentBlock::Text {
                                text: trimmed.to_string(),
                            });
                            has_text = true;
                        }
                    }
                }
            }

            if has_text {
                stats.user_messages += 1;
            }
            if !blocks.is_empty() {
                messages.push(Message {
                    role: Role::User,
                    timestamp: msg_ts,
                    model: None,
                    content: blocks,
                    usage: None,
                });
            }
        } else if role_str == "assistant" {
            let mut blocks = Vec::new();

            for part in &msg_parts {
                let pt = part.part_type.as_deref().unwrap_or("");
                match pt {
                    "text" => {
                        if let Some(text) = &part.text {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                blocks.push(ContentBlock::Text {
                                    text: trimmed.to_string(),
                                });
                                stats.assistant_messages += 1;
                            }
                        }
                    }
                    "tool" => {
                        if let Some(ref state) = part.state {
                            stats.tool_calls += 1;
                            let is_error = state
                                .metadata
                                .as_ref()
                                .and_then(|m| m.exit)
                                .is_some_and(|code| code != 0);
                            if is_error {
                                stats.tool_errors += 1;
                            }

                            let tool_name = part.tool.as_deref().unwrap_or("unknown");

                            // Track file ops
                            if let Some(ref input) = state.input {
                                let file_path = input.get("filePath").and_then(|v| v.as_str());
                                if let Some(fp) = file_path {
                                    match tool_name {
                                        "read" => { stats.files_read.insert(fp.to_string()); }
                                        "write" => { stats.files_written.insert(fp.to_string()); }
                                        _ => {}
                                    }
                                }
                            }

                            let desc = state
                                .title
                                .as_deref()
                                .or(state.metadata.as_ref().and_then(|m| m.description.as_deref()))
                                .unwrap_or("");
                            let truncated = if desc.len() > 100 {
                                format!("{}...", &desc[..100])
                            } else {
                                desc.to_string()
                            };

                            blocks.push(ContentBlock::ToolResult {
                                tool_use_id: String::new(),
                                name: tool_name.to_string(),
                                success: !is_error,
                                summary: truncated,
                            });
                        }
                    }
                    _ => {}
                }
            }

            if !blocks.is_empty() {
                let usage = oc_msg.tokens.as_ref().map(|t| TokenUsage {
                    input_tokens: t.input.unwrap_or(0),
                    output_tokens: t.output.unwrap_or(0),
                    cache_read_tokens: t.cache.as_ref().and_then(|c| c.read).unwrap_or(0),
                    cache_creation_tokens: t.cache.as_ref().and_then(|c| c.write).unwrap_or(0),
                });

                messages.push(Message {
                    role: Role::Assistant,
                    timestamp: msg_ts,
                    model: current_model.clone(),
                    content: blocks,
                    usage,
                });
            }
        }
    }

    metadata.model = current_model;

    if total_cost > 0.0 {
        stats.cost = Some(total_cost);
    }

    if let Some(last) = last_timestamp {
        let dur = (last - started_at).num_seconds();
        if dur >= 0 {
            stats.duration_seconds = Some(dur);
        }
    }

    Ok(Session {
        metadata,
        messages,
        stats,
    })
}

fn find_session_file(storage_dir: &Path, session_id: &str) -> Result<OCSession, CassioError> {
    let session_dir = storage_dir.join("session");
    if session_dir.is_dir() {
        for entry in std::fs::read_dir(&session_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let session_file = entry.path().join(format!("{session_id}.json"));
                if session_file.exists() {
                    let content = std::fs::read_to_string(&session_file)?;
                    let session: OCSession = serde_json::from_str(&content).map_err(|e| {
                        CassioError::Json {
                            path: session_file,
                            source: e,
                        }
                    })?;
                    return Ok(session);
                }
            }
        }
    }

    // Fallback: construct minimal session data
    Ok(OCSession {
        id: session_id.to_string(),
        directory: None,
        title: None,
        time: None,
    })
}

fn load_messages(dir: &Path) -> Result<Vec<OCMessage>, CassioError> {
    let mut messages = Vec::new();
    if !dir.is_dir() {
        return Ok(messages);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            let content = std::fs::read_to_string(&path)?;
            match serde_json::from_str::<OCMessage>(&content) {
                Ok(msg) => messages.push(msg),
                Err(_) => continue,
            }
        }
    }
    Ok(messages)
}

fn load_parts(dir: &Path) -> Result<Vec<OCPart>, CassioError> {
    let mut parts = Vec::new();
    if !dir.is_dir() {
        return Ok(parts);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "json") {
            let content = std::fs::read_to_string(&path)?;
            match serde_json::from_str::<OCPart>(&content) {
                Ok(part) => parts.push(part),
                Err(_) => continue,
            }
        }
    }
    Ok(parts)
}

fn timestamp_from_millis(ms: i64) -> DateTime<Utc> {
    let secs = ms / 1000;
    let nsecs = ((ms % 1000) * 1_000_000) as u32;
    Utc.timestamp_opt(secs, nsecs).single().unwrap_or_else(Utc::now)
}
