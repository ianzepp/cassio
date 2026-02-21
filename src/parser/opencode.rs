//! Parser for OpenCode session logs (fragmented JSON storage layout).
//!
//! # System context
//!
//! OpenCode's storage layout is radically different from Claude and Codex. Instead
//! of a single JSONL file per session, data is fragmented across three directory
//! hierarchies under `~/.local/share/opencode/storage/`:
//!
//! ```text
//! storage/
//!   session/<project_id>/<session_id>.json    — session metadata
//!   message/<session_id>/<message_id>.json    — one file per message
//!   part/<message_id>/<part_id>.json          — one file per content part
//! ```
//!
//! The session ID is an opaque `ses_*` string. Messages and parts are also
//! opaque IDs with no embedded timestamp, so the parser sorts messages by their
//! `time.created` field after loading them all.
//!
//! # Design philosophy
//!
//! The parser uses three loading phases:
//! 1. Load session metadata from the `session/` tree
//! 2. Load all messages from `message/<ses_id>/` and sort by creation time
//! 3. For each message, load its parts from `part/<msg_id>/`
//!
//! Parts (the actual content blocks) are processed inline while iterating sorted
//! messages, so the final message list is already in chronological order.
//!
//! # Entry point
//!
//! `OpenCodeParser::parse_session` accepts two kinds of paths:
//! - A path ending in `message/ses_*` — directly identifies a session directory
//! - A storage root directory — the parser enumerates sessions and parses the first
//!
//! The discover module always produces `message/ses_*` paths, so the storage-root
//! path is mainly for manual or test use.
//!
//! # TRADE-OFFS
//!
//! - Loading all message files eagerly (rather than streaming) simplifies the sort
//!   step. For very large sessions (thousands of messages) this could use significant
//!   memory. In practice OpenCode sessions are short.
//! - When no session metadata file is found, the parser falls back to a minimal
//!   `OCSession` with just the ID rather than failing. This handles the case where
//!   the `session/` directory is missing or the project subdirectory name is unknown.
//! - Timestamps in OpenCode are Unix milliseconds stored as `f64`, not ISO strings.
//!   `timestamp_from_millis` converts them to `DateTime<Utc>`.

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::ast::*;
use crate::error::CassioError;
use crate::parser::Parser;

/// Parser for OpenCode's fragmented JSON session storage.
pub struct OpenCodeParser;

impl Parser for OpenCodeParser {
    /// Parse a single OpenCode session.
    ///
    /// Accepts either:
    /// - A `message/ses_*` path (the canonical form produced by discovery), or
    /// - A storage root directory (enumerates sessions and parses the first one found)
    fn parse_session(&self, path: &Path) -> Result<Session, CassioError> {
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

// ── OpenCode JSON data structures ────────────────────────────────────────────
//
// These structs model OpenCode's on-disk JSON format. All fields are `Option`
// because OpenCode's schema evolves and we want to degrade gracefully when
// fields are absent rather than failing the whole parse.

/// Session metadata from `session/<project_id>/<session_id>.json`.
#[derive(Deserialize)]
struct OCSession {
    id: String,
    /// Working directory for the session (the user's project path).
    directory: Option<String>,
    title: Option<String>,
    time: Option<OCTime>,
}

/// Unix millisecond timestamps recorded by OpenCode.
#[derive(Deserialize)]
#[allow(dead_code)]
struct OCTime {
    created: Option<f64>,
    updated: Option<f64>,
}

/// A single conversation message from `message/<session_id>/<message_id>.json`.
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

/// Per-message timing; `completed` is used when available, falling back to `created`.
#[derive(Deserialize)]
struct OCMsgTime {
    created: Option<f64>,
    completed: Option<f64>,
}

/// Token usage recorded per message by OpenCode.
#[derive(Deserialize)]
struct OCTokens {
    input: Option<u64>,
    output: Option<u64>,
    cache: Option<OCCache>,
}

/// Cache token breakdown; `write` corresponds to `cache_creation_tokens` in the AST.
#[derive(Deserialize)]
struct OCCache {
    read: Option<u64>,
    write: Option<u64>,
}

/// A content part from `part/<message_id>/<part_id>.json`.
///
/// Parts are the leaf content units. A single message may have many parts:
/// text paragraphs, tool invocations, and tool outcomes are all separate part files.
#[derive(Deserialize)]
struct OCPart {
    #[serde(rename = "type")]
    part_type: Option<String>,
    text: Option<String>,
    /// When `true`, this text part was injected by OpenCode (e.g., context blocks)
    /// and should not appear in the transcript.
    synthetic: Option<bool>,
    /// Tool name for `type: "tool"` parts.
    tool: Option<String>,
    state: Option<OCPartState>,
}

/// Outcome state for a tool part.
#[derive(Deserialize)]
#[allow(dead_code)]
struct OCPartState {
    status: Option<String>,
    input: Option<Value>,
    title: Option<String>,
    metadata: Option<OCPartMeta>,
}

/// OS-level metadata for a completed tool execution.
#[derive(Deserialize)]
struct OCPartMeta {
    /// Exit code; non-zero values indicate failure.
    exit: Option<i32>,
    description: Option<String>,
}

/// Load and assemble a complete session from the OpenCode storage layout.
///
/// PHASE 1: SESSION METADATA
/// Search the `session/` tree for `<session_id>.json`. Fall back to a minimal
/// placeholder if not found (handles cases where the project directory is unknown).
///
/// PHASE 2: MESSAGE LOADING AND SORTING
/// Read all `*.json` files from `message/<session_id>/`. Sort by `time.created`
/// because the filesystem does not guarantee any particular order.
///
/// PHASE 3: PART LOADING
/// For each message, eagerly load all parts from `part/<message_id>/` into a
/// `HashMap<message_id, Vec<OCPart>>`. This pre-loads all content before AST
/// construction, keeping the AST-building loop simple.
///
/// PHASE 4: AST CONSTRUCTION
/// Iterate sorted messages, emitting `ModelChange`, `User`, and `Assistant`
/// message nodes. Tool stats and token totals are accumulated here.
fn parse_session(storage_dir: &Path, session_id: &str) -> Result<Session, CassioError> {
    // PHASE 1: SESSION METADATA
    let session_data = find_session_file(storage_dir, session_id)?;

    // PHASE 2: MESSAGE LOADING AND SORTING
    let messages_dir = storage_dir.join("message").join(session_id);
    let mut oc_messages = load_messages(&messages_dir)?;
    oc_messages.sort_by(|a, b| {
        let ta = a.time.as_ref().and_then(|t| t.created).unwrap_or(0.0);
        let tb = b.time.as_ref().and_then(|t| t.created).unwrap_or(0.0);
        ta.partial_cmp(&tb).unwrap_or(std::cmp::Ordering::Equal)
    });

    // PHASE 3: PART LOADING
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

    // PHASE 4: AST CONSTRUCTION
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
            let mut has_text = false;

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
                                has_text = true;
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
                                format!("{}...", super::truncate(desc, 100))
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
                if has_text {
                    stats.assistant_messages += 1;
                }

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

/// Search the `session/<project_id>/` tree for a session metadata file.
///
/// WHY: OpenCode nests session files under an opaque project ID directory that
/// cassio does not know in advance. The search iterates all project subdirectories
/// to find the matching `<session_id>.json`.
///
/// Falls back to a minimal placeholder when no file is found rather than erroring,
/// so that sessions with no metadata file (e.g., very old or incomplete sessions)
/// can still be parsed from their messages and parts.
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

    // EDGE: No session file found — return a minimal placeholder so parsing can continue.
    Ok(OCSession {
        id: session_id.to_string(),
        directory: None,
        title: None,
        time: None,
    })
}

/// Load all message JSON files from a session's message directory.
///
/// Files that fail to deserialize as `OCMessage` are silently skipped. This
/// handles malformed or partial writes without aborting the parse.
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

/// Load all content part JSON files for a single message.
///
/// Parts that fail to deserialize are silently skipped.
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

/// Convert an OpenCode Unix millisecond timestamp to `DateTime<Utc>`.
///
/// WHY: OpenCode stores timestamps as `f64` milliseconds rather than ISO strings.
/// Using `div_euclid` / `rem_euclid` rather than plain division ensures correct
/// handling of negative timestamps (pre-epoch) without panicking on overflow.
fn timestamp_from_millis(ms: i64) -> DateTime<Utc> {
    let secs = ms.div_euclid(1000);
    let nsecs = (ms.rem_euclid(1000) * 1_000_000) as u32;
    Utc.timestamp_opt(secs, nsecs).single().unwrap_or_else(Utc::now)
}
