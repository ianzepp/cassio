//! Parser for Claude Chat privacy exports (`conversations.json`).
//!
//! # System context
//!
//! Claude.ai / Claude Desktop Chat history is not stored as local Code JSONL.
//! Anthropic's official path is Settings → Privacy → Export data, which yields a
//! zip (or extracted tree) containing:
//!
//! - `conversations.json` — array of chat conversations
//! - `users.json` — account profile (ignored by cassio)
//! - `projects/*.json` — Project metadata (ignored; chats embed their own messages)
//!
//! Each conversation object has `uuid`, `name`, timestamps, and `chat_messages[]`
//! with `sender` (`human` / `assistant`) and Anthropic-style `content[]` blocks.
//!
//! # Virtual paths
//!
//! Batch discovery maps each conversation to a virtual path:
//!
//! ```text
//! <export>/conversations.json/<uuid>
//! ```
//!
//! where `<export>` is a `.zip`, a directory containing `conversations.json`, or
//! the `conversations.json` file itself. The parser reloads the export and selects
//! the matching conversation by UUID.
//!
//! # TRADE-OFFS
//!
//! - Tool `id` / `tool_use_id` may be null in the export; cassio synthesizes stable
//!   placeholders so ToolUse/ToolResult pairing still works in the AST.
//! - `token_budget` and other non-transcript blocks are dropped.
//! - Projects docs are not imported as sessions — only `conversations.json`.

use std::fs;
use std::io::Read;
#[cfg(test)]
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use zip::ZipArchive;

use crate::ast::*;
use crate::error::CassioError;
use crate::parser::Parser;
use crate::parser::claude::format_tool_input;
use crate::training::{
    ParsedSession, TrainingEvent, TrainingMetadata, TrainingSession, TrainingSource,
    hash_named_chunks, next_event_id, training_stats_from_session,
};

const VIRTUAL_MARKER: &str = "conversations.json/";

/// Parser for Claude Chat privacy-export conversations.
pub struct ClaudeChatParser;

impl Parser for ClaudeChatParser {
    fn parse_export(&self, path: &Path) -> Result<ParsedSession, CassioError> {
        let (export_path, session_id) = split_virtual_path(path).ok_or_else(|| {
            CassioError::Other(format!(
                "Claude Chat path must look like <export>/conversations.json/<uuid>: {}",
                path.display()
            ))
        })?;
        let bytes = read_conversations_bytes(&export_path)?;
        let conversations: Vec<RawConversation> = serde_json::from_slice(&bytes).map_err(|e| {
            CassioError::Other(format!(
                "Failed to parse conversations.json from {}: {e}",
                export_path.display()
            ))
        })?;
        let raw = conversations
            .into_iter()
            .find(|c| c.uuid == session_id)
            .ok_or_else(|| {
                CassioError::Other(format!(
                    "Conversation {session_id} not found in {}",
                    export_path.display()
                ))
            })?;
        let conv_bytes = serde_json::to_vec(&raw).unwrap_or_default();
        normalize(
            raw,
            path.to_string_lossy().to_string(),
            Some(export_path.to_string_lossy().to_string()),
            &conv_bytes,
        )
    }
}

/// Discover all Claude Chat sessions in a privacy export path.
///
/// Accepts a `.zip`, a directory containing `conversations.json`, or the JSON file.
/// Returns virtual paths suitable for `ClaudeChatParser`.
pub fn discover_export_sessions(export: &Path) -> Result<Vec<(Tool, PathBuf)>, CassioError> {
    let export = resolve_export_path(export)?;
    let conversations = load_raw_conversations(&export)?;

    let mut out = Vec::with_capacity(conversations.len());
    for conv in conversations {
        if conv.uuid.is_empty() {
            continue;
        }
        out.push((Tool::ClaudeChat, virtual_path(&export, &conv.uuid)));
    }
    Ok(out)
}

/// Load and parse every conversation from a privacy export **once**.
///
/// Prefer this over repeatedly calling `ClaudeChatParser::parse_export` on virtual
/// paths when batch-importing a zip (each virtual parse would re-read the archive).
pub fn parse_export_all(export: &Path) -> Result<Vec<ParsedSession>, CassioError> {
    let export = resolve_export_path(export)?;
    let conversations = load_raw_conversations(&export)?;
    let export_root = export.to_string_lossy().to_string();
    let mut out = Vec::with_capacity(conversations.len());
    for raw in conversations {
        if raw.uuid.is_empty() {
            continue;
        }
        let source_path = virtual_path(&export, &raw.uuid)
            .to_string_lossy()
            .to_string();
        let conv_bytes = serde_json::to_vec(&raw).unwrap_or_default();
        out.push(normalize(
            raw,
            source_path,
            Some(export_root.clone()),
            &conv_bytes,
        )?);
    }
    Ok(out)
}

fn load_raw_conversations(export: &Path) -> Result<Vec<RawConversation>, CassioError> {
    let bytes = read_conversations_bytes(export)?;
    serde_json::from_slice(&bytes).map_err(|e| {
        CassioError::Other(format!(
            "Failed to parse conversations.json from {}: {e}",
            export.display()
        ))
    })
}

/// Build the virtual path for a conversation UUID within an export.
pub fn virtual_path(export: &Path, uuid: &str) -> PathBuf {
    let name = export
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.eq_ignore_ascii_case("conversations.json") {
        // export is the JSON file itself → …/conversations.json/<uuid>
        export.join(uuid)
    } else {
        // zip or directory → …/export/conversations.json/<uuid>
        export.join("conversations.json").join(uuid)
    }
}

/// Return the real export path used for mtime checks (zip / json / directory root).
pub fn export_root_from_virtual(path: &Path) -> Option<PathBuf> {
    split_virtual_path(path).map(|(export, _)| export)
}

/// Parse a single conversation object (for tests and direct callers).
pub fn parse_conversation_json(
    value: &Value,
    source_path: impl Into<String>,
) -> Result<ParsedSession, CassioError> {
    let raw: RawConversation = serde_json::from_value(value.clone())
        .map_err(|e| CassioError::Other(format!("Invalid Claude Chat conversation: {e}")))?;
    let source = source_path.into();
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    normalize(raw, source, None, &bytes)
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct RawConversation {
    uuid: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    created_at: String,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    chat_messages: Vec<RawChatMessage>,
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct RawChatMessage {
    #[serde(default)]
    uuid: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    content: Value,
    sender: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

fn split_virtual_path(path: &Path) -> Option<(PathBuf, String)> {
    let path_str = path.to_string_lossy();
    let (prefix, uuid) = path_str.split_once(VIRTUAL_MARKER)?;
    let uuid = uuid.trim_matches('/');
    if uuid.is_empty() || uuid.contains('/') {
        return None;
    }
    // Prefer the conversations.json file when it exists (export is the JSON or a dir).
    let json_file = PathBuf::from(format!("{prefix}conversations.json"));
    if json_file.is_file() {
        return Some((json_file, uuid.to_string()));
    }
    // Otherwise the export is a zip (or missing tree) at the prefix path.
    let export = if prefix.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(prefix.trim_end_matches('/'))
    };
    Some((export, uuid.to_string()))
}

fn resolve_export_path(export: &Path) -> Result<PathBuf, CassioError> {
    if !export.exists() {
        return Err(CassioError::Other(format!(
            "Claude Chat export not found: {}",
            export.display()
        )));
    }
    Ok(export.to_path_buf())
}

fn read_conversations_bytes(export: &Path) -> Result<Vec<u8>, CassioError> {
    if export.is_file() {
        let name = export
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.eq_ignore_ascii_case("conversations.json") {
            return fs::read(export).map_err(CassioError::from);
        }
        if name.ends_with(".zip") || is_zip_file(export) {
            return read_conversations_from_zip(export);
        }
        // Bare JSON file that is not named conversations.json — try as the array itself.
        let bytes = fs::read(export)?;
        if looks_like_conversations_json(&bytes) {
            return Ok(bytes);
        }
        return Err(CassioError::Other(format!(
            "Not a Claude Chat export (expected .zip or conversations.json): {}",
            export.display()
        )));
    }

    if export.is_dir() {
        let json_path = export.join("conversations.json");
        if json_path.is_file() {
            return fs::read(json_path).map_err(CassioError::from);
        }
        return Err(CassioError::Other(format!(
            "Directory has no conversations.json: {}",
            export.display()
        )));
    }

    Err(CassioError::Other(format!(
        "Claude Chat export path is neither file nor directory: {}",
        export.display()
    )))
}

fn is_zip_file(path: &Path) -> bool {
    fs::File::open(path)
        .ok()
        .and_then(|mut f| {
            let mut magic = [0u8; 4];
            f.read_exact(&mut magic).ok()?;
            Some(&magic == b"PK\x03\x04" || &magic == b"PK\x05\x06" || &magic == b"PK\x07\x08")
        })
        .unwrap_or(false)
}

fn looks_like_conversations_json(bytes: &[u8]) -> bool {
    let trimmed = std::str::from_utf8(bytes)
        .map(|s| s.trim_start())
        .unwrap_or("");
    trimmed.starts_with('[')
}

fn read_conversations_from_zip(path: &Path) -> Result<Vec<u8>, CassioError> {
    let file = fs::File::open(path)?;
    let mut archive = ZipArchive::new(file)
        .map_err(|e| CassioError::Other(format!("Failed to open zip {}: {e}", path.display())))?;

    // Prefer a top-level conversations.json; also accept nested paths.
    let mut candidate_index: Option<usize> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| {
            CassioError::Other(format!("Failed to read zip entry in {}: {e}", path.display()))
        })?;
        let name = entry.name().replace('\\', "/");
        if name == "conversations.json" || name.ends_with("/conversations.json") {
            candidate_index = Some(i);
            if name == "conversations.json" {
                break;
            }
        }
    }

    let index = candidate_index.ok_or_else(|| {
        CassioError::Other(format!(
            "Zip has no conversations.json: {}",
            path.display()
        ))
    })?;

    let mut entry = archive.by_index(index).map_err(|e| {
        CassioError::Other(format!(
            "Failed to open conversations.json in {}: {e}",
            path.display()
        ))
    })?;
    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|e| CassioError::Other(format!("Failed to read conversations.json: {e}")))?;
    Ok(bytes)
}

fn normalize(
    raw: RawConversation,
    source_path: String,
    source_root: Option<String>,
    source_bytes: &[u8],
) -> Result<ParsedSession, CassioError> {
    let started_at = parse_timestamp(&raw.created_at).unwrap_or_else(Utc::now);
    let ended_at = raw
        .updated_at
        .as_deref()
        .and_then(parse_timestamp)
        .or_else(|| {
            raw.chat_messages
                .iter()
                .rev()
                .find_map(|m| m.created_at.as_deref().and_then(parse_timestamp))
        });

    let title = raw
        .name
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let mut messages: Vec<Message> = Vec::new();
    let mut stats = SessionStats::default();
    let mut training_events: Vec<TrainingEvent> = Vec::new();
    let mut sequence: u64 = 0;
    let mut pending_tools: std::collections::HashMap<String, (String, Value)> =
        std::collections::HashMap::new();
    let mut synthetic_tool_seq: u64 = 0;
    let mut first_ts: Option<DateTime<Utc>> = Some(started_at);
    let mut last_ts: Option<DateTime<Utc>> = ended_at;

    if let Some(summary) = raw
        .summary
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        // Surface non-empty Anthropic summary as a system note once.
        messages.push(Message {
            role: Role::System,
            timestamp: Some(started_at),
            model: None,
            content: vec![ContentBlock::Text {
                text: format!("Summary: {summary}"),
            }],
            usage: None,
        });
        sequence += 1;
        training_events.push(TrainingEvent {
            event_id: next_event_id(sequence),
            sequence,
            timestamp: Some(started_at),
            role: Some("system".to_string()),
            event_kind: "summary".to_string(),
            model: None,
            raw_text: Some(summary.to_string()),
            sanitized_text: None,
            tool_name: None,
            tool_call_id: None,
            tool_input_raw: None,
            tool_input_sanitized: None,
            tool_output_raw: None,
            tool_output_sanitized: None,
            usage: None,
            source_record_refs: vec!["conversation.summary".to_string()],
        });
    }

    for (msg_index, msg) in raw.chat_messages.iter().enumerate() {
        let ts = msg
            .created_at
            .as_deref()
            .and_then(parse_timestamp)
            .or_else(|| msg.updated_at.as_deref().and_then(parse_timestamp));
        if let Some(t) = ts {
            if first_ts.is_none() {
                first_ts = Some(t);
            }
            last_ts = Some(t);
        }

        let role = match msg.sender.as_str() {
            "human" | "user" => Role::User,
            "assistant" => Role::Assistant,
            _ => Role::System,
        };

        let source_ref = format!(
            "chat_messages[{}]:{}",
            msg_index,
            msg.uuid.as_deref().unwrap_or("unknown")
        );

        let mut blocks: Vec<ContentBlock> = Vec::new();
        append_content_blocks(
            &msg.content,
            msg.text.as_deref(),
            role,
            &mut blocks,
            &mut stats,
            &mut pending_tools,
            &mut synthetic_tool_seq,
            &mut sequence,
            &mut training_events,
            ts,
            &source_ref,
        );

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
            timestamp: ts,
            model: None,
            content: blocks,
            usage: None,
        });
    }

    if let (Some(first), Some(last)) = (first_ts, last_ts) {
        let dur = (last - first).num_seconds();
        if dur >= 0 {
            stats.duration_seconds = Some(dur);
        }
    }

    let metadata = SessionMetadata {
        session_id: raw.uuid.clone(),
        tool: Tool::ClaudeChat,
        // Chat has no coding cwd; keep a stable non-path label for filters/summaries.
        project_path: "claude-chat".to_string(),
        started_at,
        session_kind: classify_session_kind(&messages),
        version: None,
        git_branch: None,
        model: None,
        title,
    };

    let session = Session {
        metadata: metadata.clone(),
        messages,
        stats,
    };

    let training_metadata = TrainingMetadata {
        project_path_raw: session.metadata.project_path.clone(),
        project_path_sanitized: session.metadata.project_path.clone(),
        started_at: session.metadata.started_at,
        ended_at: last_ts,
        git_branch: None,
        title: session.metadata.title.clone(),
        session_kind: session.metadata.session_kind.to_string(),
        models_seen: Vec::new(),
        version: None,
    };

    let source = TrainingSource {
        tool: session.metadata.tool.to_string(),
        source_path,
        session_id: session.metadata.session_id.clone(),
        source_hash: hash_named_chunks([(
            "conversation",
            String::from_utf8_lossy(source_bytes).into_owned(),
        )]),
        source_record_count: Some(raw.chat_messages.len() as u64),
        source_format: Some("claude-chat-export".to_string()),
        source_root,
    };

    let mut training = TrainingSession::new(
        "claude_chat.v1",
        source,
        training_metadata,
        training_stats_from_session(&session.stats),
    );
    for event in training_events {
        training.push_event(event);
    }

    Ok(ParsedSession { session, training })
}

#[allow(clippy::too_many_arguments)]
fn append_content_blocks(
    content: &Value,
    fallback_text: Option<&str>,
    role: Role,
    blocks: &mut Vec<ContentBlock>,
    stats: &mut SessionStats,
    pending_tools: &mut std::collections::HashMap<String, (String, Value)>,
    synthetic_tool_seq: &mut u64,
    sequence: &mut u64,
    training_events: &mut Vec<TrainingEvent>,
    ts: Option<DateTime<Utc>>,
    source_ref: &str,
) {
    match content {
        Value::Array(items) if !items.is_empty() => {
            for block in items {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match block_type {
                    "text" => {
                        let text = block
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        if text.is_empty() {
                            continue;
                        }
                        blocks.push(ContentBlock::Text { text: text.clone() });
                        *sequence += 1;
                        training_events.push(message_event(
                            *sequence,
                            ts,
                            role,
                            text,
                            source_ref,
                        ));
                    }
                    "thinking" => {
                        let text = block
                            .get("thinking")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .to_string();
                        if text.is_empty() {
                            continue;
                        }
                        blocks.push(ContentBlock::Thinking { text: text.clone() });
                        *sequence += 1;
                        training_events.push(TrainingEvent {
                            event_id: next_event_id(*sequence),
                            sequence: *sequence,
                            timestamp: ts,
                            role: Some(role_str(role).to_string()),
                            event_kind: "thinking".to_string(),
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
                            source_record_refs: vec![source_ref.to_string()],
                        });
                    }
                    "tool_use" => {
                        let name = block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let input = block.get("input").cloned().unwrap_or(Value::Null);
                        let id = block
                            .get("id")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| {
                                *synthetic_tool_seq += 1;
                                format!("claude-chat-tool-{synthetic_tool_seq}")
                            });
                        pending_tools.insert(id.clone(), (name.clone(), input.clone()));
                        stats.tool_calls += 1;
                        track_files_from_tool(&name, &input, stats);
                        blocks.push(ContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        });
                        *sequence += 1;
                        training_events.push(TrainingEvent {
                            event_id: next_event_id(*sequence),
                            sequence: *sequence,
                            timestamp: ts,
                            role: Some(role_str(role).to_string()),
                            event_kind: "tool_use".to_string(),
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
                            source_record_refs: vec![source_ref.to_string()],
                        });
                    }
                    "tool_result" => {
                        let tool_use_id = block
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string());
                        let is_error = block
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if is_error {
                            stats.tool_errors += 1;
                        }

                        let (name, input) = if let Some(ref id) = tool_use_id {
                            pending_tools
                                .remove(id)
                                .unwrap_or_else(|| {
                                    (
                                        block
                                            .get("name")
                                            .and_then(|n| n.as_str())
                                            .unwrap_or("unknown")
                                            .to_string(),
                                        Value::Null,
                                    )
                                })
                        } else {
                            (
                                block
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or("unknown")
                                    .to_string(),
                                Value::Null,
                            )
                        };

                        let id = tool_use_id.unwrap_or_else(|| {
                            *synthetic_tool_seq += 1;
                            format!("claude-chat-tool-result-{synthetic_tool_seq}")
                        });

                        let summary = if input.is_null() {
                            summarize_tool_result_content(block.get("content"))
                        } else {
                            format_tool_input(&name, &input)
                        };

                        blocks.push(ContentBlock::ToolResult {
                            tool_use_id: id.clone(),
                            name: name.clone(),
                            success: !is_error,
                            summary,
                        });

                        *sequence += 1;
                        training_events.push(TrainingEvent {
                            event_id: next_event_id(*sequence),
                            sequence: *sequence,
                            timestamp: ts,
                            role: Some(role_str(role).to_string()),
                            event_kind: "tool_result".to_string(),
                            model: None,
                            raw_text: None,
                            sanitized_text: None,
                            tool_name: Some(name),
                            tool_call_id: Some(id),
                            tool_input_raw: None,
                            tool_input_sanitized: None,
                            tool_output_raw: block.get("content").cloned(),
                            tool_output_sanitized: None,
                            usage: None,
                            source_record_refs: vec![source_ref.to_string()],
                        });
                    }
                    // token_budget and unknown block types are intentionally ignored.
                    _ => {}
                }
            }
        }
        Value::String(text) if !text.is_empty() => {
            blocks.push(ContentBlock::Text {
                text: text.clone(),
            });
            *sequence += 1;
            training_events.push(message_event(*sequence, ts, role, text.clone(), source_ref));
        }
        _ => {
            if let Some(text) = fallback_text.map(str::trim).filter(|s| !s.is_empty()) {
                blocks.push(ContentBlock::Text {
                    text: text.to_string(),
                });
                *sequence += 1;
                training_events.push(message_event(
                    *sequence,
                    ts,
                    role,
                    text.to_string(),
                    source_ref,
                ));
            }
        }
    }
}

fn message_event(
    sequence: u64,
    ts: Option<DateTime<Utc>>,
    role: Role,
    text: String,
    source_ref: &str,
) -> TrainingEvent {
    TrainingEvent {
        event_id: next_event_id(sequence),
        sequence,
        timestamp: ts,
        role: Some(role_str(role).to_string()),
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
        source_record_refs: vec![source_ref.to_string()],
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
    }
}

fn summarize_tool_result_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => {
            if s.len() > 150 {
                format!("{}...", crate::parser::truncate(s, 150))
            } else {
                s.clone()
            }
        }
        Some(Value::Array(items)) => {
            let texts: Vec<&str> = items
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .and_then(|t| t.as_str())
                        .or_else(|| item.as_str())
                })
                .collect();
            let joined = texts.join(" ");
            if joined.is_empty() {
                "tool result".to_string()
            } else if joined.len() > 150 {
                format!("{}...", crate::parser::truncate(&joined, 150))
            } else {
                joined
            }
        }
        Some(other) => {
            let s = other.to_string();
            if s.len() > 150 {
                format!("{}...", crate::parser::truncate(&s, 150))
            } else {
                s
            }
        }
        None => "tool result".to_string(),
    }
}

fn track_files_from_tool(name: &str, input: &Value, stats: &mut SessionStats) {
    let path = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(|v| v.as_str());
    let Some(path) = path else {
        return;
    };
    match name {
        "Read" => {
            stats.files_read.insert(path.to_string());
        }
        "Write" => {
            stats.files_written.insert(path.to_string());
        }
        "Edit" => {
            stats.files_edited.insert(path.to_string());
        }
        _ => {}
    }
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    s.parse::<DateTime<Utc>>().ok()
}

/// Write conversations.json bytes into a zip at `path` (test helper surface).
#[cfg(test)]
pub fn write_test_zip(path: &Path, conversations_json: &[u8]) -> Result<(), CassioError> {
    let file = fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored);
    zip.start_file("conversations.json", options)
        .map_err(|e| CassioError::Other(format!("zip start_file: {e}")))?;
    zip.write_all(conversations_json)
        .map_err(|e| CassioError::Other(format!("zip write: {e}")))?;
    zip.finish()
        .map_err(|e| CassioError::Other(format!("zip finish: {e}")))?;
    Ok(())
}

#[cfg(test)]
#[path = "claude_chat_test.rs"]
mod tests;
