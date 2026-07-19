//! Parser for Kimi Code session logs (`~/.kimi-code/sessions/**/session_*/agents/*/wire.jsonl`).
//!
//! Kimi Code stores sessions under `~/.kimi-code/sessions/<workdir-hash>/session_<uuid>/`.
//! Each session has a `state.json` with metadata (workDir, agents, timestamps), and one or
//! more agent directories under `agents/` — each containing a `wire.jsonl` with the full
//! conversation stream.
//!
//! The wire format is a JSONL stream of typed records:
//! - `metadata` — protocol version and creation time
//! - `config.update` — model alias, system prompt, permissions
//! - `turn.prompt` — user input (`input` array of `{type:"text", text}`)
//! - `context.append_loop_event` — the bulk carrier: tool calls, tool results,
//!   thinking blocks, text replies, step begin/end with usage
//! - `context.append_message` — system/injected messages
//! - `usage.record` — token usage per turn
//! - `llm.request` — request metadata (model, message count)

use std::io::BufRead;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ast::*;
use crate::ast::TokenUsage;
use crate::error::CassioError;
use crate::parser::Parser;
use crate::parser::claude::format_tool_input;
use crate::training::{
    ParsedSession, TrainingEvent, TrainingMetadata, TrainingSession, TrainingSource,
    hash_named_chunks, next_event_id, training_stats_from_session,
};

pub struct KimiCodeParser;

/// Extract the session ID from a source path.
///
/// The source path is `agents/<agent>/wire.jsonl`, so the session directory is
/// the great-grandparent (e.g. `session_236b3acc...` under `wd_faberlang_xxx/`).
pub fn kimi_session_id_from_source(path: &Path) -> Option<String> {
    path.parent()         // agents/main/wire.jsonl → agents/main/
        .and_then(|p| p.parent())  // → agents/
        .and_then(|p| p.parent())  // → session_abc/
        .and_then(|d| d.file_name())
        .and_then(|n| n.to_str())
        .map(str::to_string)
}

/// Extract the session start time from a source path by reading `state.json`.
///
/// Returns `None` if the state file is absent or unparseable.
pub fn kimi_started_at_from_source(path: &Path) -> Option<DateTime<Utc>> {
    let session_dir = path.parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())?;
    let state_path = session_dir.join("state.json");
    let content = std::fs::read_to_string(state_path).ok()?;
    let state: Value = serde_json::from_str(&content).ok()?;
    let created_at = state
        .get("createdAt")
        .and_then(|v| v.as_str())?;
    created_at.parse::<DateTime<Utc>>().ok()
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct KimiStateFile {
    #[serde(default, rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "workDir")]
    pub work_dir: Option<String>,
}

impl Parser for KimiCodeParser {
    fn parse_export(&self, path: &Path) -> Result<ParsedSession, CassioError> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;
        parse_lines(
            lines.into_iter(),
            path.to_string_lossy().to_string(),
            path.parent()
                .and_then(|p| p.parent())
                .map(|p| p.to_string_lossy().to_string()),
        )
    }
}

impl KimiCodeParser {
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
    let mut pending_tools: std::collections::HashMap<String, (String, Value)> =
        std::collections::HashMap::new();
    let first_timestamp = metadata.as_ref().map(|m| m.started_at);
    let mut last_timestamp = first_timestamp;
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
            "metadata" | "tools.set_active_tools" | "llm.tools_snapshot" => {
                // Protocol-level records; no user-visible content
            }
            "config.update" => {
                // Track model changes from config updates
                if let Some(model_alias) =
                    record.get("modelAlias").and_then(|v| v.as_str())
                {
                    let model = model_alias.to_string();
                    if !models_seen.iter().any(|s| s == &model) {
                        models_seen.push(model.clone());
                    }
                    current_model = Some(model.clone());
                    if let Some(meta) = metadata.as_mut() {
                        meta.model = Some(model);
                    }
                }
            }
            "turn.prompt" => {
                if let Some(input) = record.get("input").and_then(|v| v.as_array()) {
                    let text = input
                        .iter()
                        .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let trimmed_text = text.trim();
                    if !trimmed_text.is_empty() {
                        stats.user_messages += 1;
                        sequence += 1;
                        messages.push(Message {
                            role: Role::User,
                            timestamp: None,
                            model: None,
                            content: vec![ContentBlock::Text {
                                text: trimmed_text.to_string(),
                            }],
                            usage: None,
                        });
                        training_events.push(TrainingEvent {
                            event_id: next_event_id(sequence),
                            sequence,
                            timestamp: None,
                            role: Some("user".to_string()),
                            event_kind: "message".to_string(),
                            model: None,
                            raw_text: Some(trimmed_text.to_string()),
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
                }
            }
            "context.append_loop_event" => {
                if let Some(event) = record.get("event").and_then(|v| v.as_object()) {
                    let event_type = event
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    match event_type {
                        "content.part" => {
                            if let Some(part) = event.get("part").and_then(|v| v.as_object()) {
                                let part_type = part
                                    .get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let time = record.get("time").and_then(|v| v.as_i64());
                                let timestamp = time_to_timestamp(time);

                                if part_type == "text" {
                                    if let Some(text) =
                                        part.get("text").and_then(|v| v.as_str())
                                    {
                                        let text = text.to_string();
                                        let blocks = if !text.trim().is_empty() {
                                            vec![ContentBlock::Text { text }]
                                        } else {
                                            Vec::new()
                                        };
                                        if !blocks.is_empty() {
                                            stats.assistant_messages += 1;
                                            messages.push(Message {
                                                role: Role::Assistant,
                                                timestamp,
                                                model: current_model.clone(),
                                                content: blocks,
                                                usage: None,
                                            });
                                        }
                                    }
                                } else if part_type == "think" {
                                    if let Some(think) =
                                        part.get("think").and_then(|v| v.as_str()).filter(|t| !t.trim().is_empty())
                                    {
                                        stats.assistant_messages += 1;
                                        messages.push(Message {
                                            role: Role::Assistant,
                                            timestamp,
                                            model: current_model.clone(),
                                            content: vec![ContentBlock::Thinking {
                                                text: think.to_string(),
                                            }],
                                            usage: None,
                                        });
                                    }
                                }
                            }
                        }
                        "tool.call" => {
                            let tool_name = event
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let tool_call_id = event
                                .get("toolCallId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let args = event
                                .get("args")
                                .cloned()
                                .unwrap_or(Value::Object(Default::default()));
                            pending_tools.insert(tool_call_id.clone(), (tool_name, args));
                        }
                        "tool.result" => {
                            let tool_call_id = event
                                .get("toolCallId")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let (name, args) = pending_tools
                                .remove(&tool_call_id)
                                .unwrap_or((
                                    "unknown".to_string(),
                                    Value::Object(Default::default()),
                                ));
                            let tool_call_id_ref = tool_call_id.clone();

                            let time = record.get("time").and_then(|v| v.as_i64());
                            let timestamp = time_to_timestamp(time);

                            let result_val = event
                                .get("result")
                                .cloned()
                                .unwrap_or(Value::Object(Default::default()));
                            let result_map = result_val.as_object().cloned().unwrap_or_default();
                            let output = result_map
                                .get("output")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let is_error = result_map
                                .get("isError")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false)
                                || grok_tool_result_failed(output);

                            if is_error {
                                stats.tool_errors += 1;
                            } else {
                                stats.tool_calls += 1;
                            }
                            track_file_ops(&mut stats, &name, &result_val);

                            let summary = format_kimi_tool_input(&name, &args);
                            messages.push(Message {
                                role: Role::Assistant,
                                timestamp,
                                model: current_model.clone(),
                                content: vec![ContentBlock::ToolResult {
                                    tool_use_id: tool_call_id,
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
                                timestamp,
                                role: Some("assistant".to_string()),
                                event_kind: "tool_result".to_string(),
                                model: current_model.clone(),
                                raw_text: None,
                                sanitized_text: None,
                                tool_name: Some(name),
                                tool_call_id: Some(tool_call_id_ref),
                                tool_input_raw: None,
                                tool_input_sanitized: None,
                                tool_output_raw: Some(json!({ "text": output })),
                                tool_output_sanitized: None,
                                usage: None,
                                source_record_refs: vec![source_ref],
                            });
                        }
                        "step.end" => {
                            if let Some(usage) =
                                event.get("usage").and_then(|v| v.as_object())
                            {
                                let input_cache_read = usage
                                    .get("inputCacheRead")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let input_cache_creation = usage
                                    .get("inputCacheCreation")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let output = usage
                                    .get("output")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let input_other = usage
                                    .get("inputOther")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);

                                // Accumulate usage into last assistant message
                                if let Some(last) = messages.last_mut() {
                                    last.usage = Some(TokenUsage {
                                        input_tokens: input_other,
                                        output_tokens: output,
                                        cache_read_tokens: input_cache_read,
                                        cache_creation_tokens: input_cache_creation,
                                    });
                                }
                            }
                            // Track end timestamp for duration calculation
                            if let Some(time) = event.get("time").and_then(|v| v.as_i64()) {
                                last_timestamp = time_to_timestamp(Some(time));
                            }
                        }
                        _ => {}
                    }
                }
            }
            "context.append_message" => {
                if let Some(msg) = record.get("message").and_then(|v| v.as_object()) {
                    let role = msg
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if role == "user" {
                        if let Some(content) = msg.get("content") {
                            let text = if let Some(arr) = content.as_array() {
                                arr.iter()
                                    .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            } else if let Some(s) = content.as_str() {
                                s.to_string()
                            } else {
                                continue;
                            };
                            let trimmed = text.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            // These are system-injected user messages (goals, reminders)
                            stats.user_messages += 1;
                            sequence += 1;
                            messages.push(Message {
                                role: Role::User,
                                timestamp: None,
                                model: None,
                                content: vec![ContentBlock::Text {
                                    text: trimmed.to_string(),
                                }],
                                usage: None,
                            });
                        }
                    }
                }
            }
            "usage.record" => {
                // Per-turn usage; captured at step.end for the last message
            }
            "llm.request" => {
                // Request metadata; model already captured via config.update
            }
            _ => {}
        }
    }

    let mut meta = metadata
        .ok_or_else(|| CassioError::Other("No Kimi Code session metadata found".into()))?;
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
        "kimi.v1",
        source,
        training_metadata,
        training_stats_from_session(&session.stats),
    );
    for event in training_events {
        training.push_event(event);
    }

    Ok(ParsedSession { session, training })
}

pub(crate) fn kimi_state_path(session_dir: &Path) -> Option<PathBuf> {
    let state_path = session_dir.join("state.json");
    if state_path.is_file() {
        Some(state_path)
    } else {
        None
    }
}

fn load_metadata_from_source(
    source_path: &str,
    source_root: Option<&str>,
) -> Option<SessionMetadata> {
    if source_path == "stdin" {
        return Some(SessionMetadata {
            session_id: "stdin".to_string(),
            tool: Tool::Kimi,
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

    // wire.jsonl → main → agents → session_<uuid>
    let session_dir = path.parent()?.parent()?.parent()?;

    // Read state.json for metadata
    let state = kimi_state_path(session_dir)
        .and_then(|sp| {
            std::fs::read_to_string(sp).ok().and_then(|c| {
                serde_json::from_str::<KimiStateFile>(&c).ok()
            })
        })
        .unwrap_or_default();

    let session_id = session_dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "unknown".to_string());

    let project_path = state.work_dir.clone().unwrap_or_default();
    let title = state.title.clone();

    let started_at = state
        .created_at
        .as_ref()
        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
        .unwrap_or_else(Utc::now);

    Some(SessionMetadata {
        session_id,
        tool: Tool::Kimi,
        project_path,
        started_at,
        session_kind: SessionKind::Uncertain,
        version: None,
        git_branch: None,
        model: None, // Model comes from wire.jsonl config.update records
        title,
    })
}

fn time_to_timestamp(ms: Option<i64>) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(ms?)
}

fn grok_tool_result_failed(output: &str) -> bool {
    for line in output.lines() {
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
        if trimmed.starts_with("Command failed with exit code:") {
            return true;
        }
    }
    false
}

fn track_file_ops(stats: &mut SessionStats, tool_name: &str, result: &Value) {
    let output = result
        .get("output")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || grok_tool_result_failed(output);

    if is_error {
        return;
    }

    let Some(path) = extract_file_path(tool_name, result) else {
        return;
    };

    match tool_name {
        "Read" | "read" => {
            stats.files_read.insert(path);
        }
        "Write" | "write" => {
            stats.files_written.insert(path);
        }
        "Edit" | "edit" => {
            stats.files_edited.insert(path);
        }
        _ => {}
    }
}

/// Format tool input for Kimi Code, which uses `path` instead of `file_path`.
///
/// WHY: Kimi Code's tool schemas use `path` for file operations, while `format_tool_input`
/// from the Claude parser expects `file_path`. This wrapper normalizes that difference.
fn format_kimi_tool_input(tool_name: &str, input: &Value) -> String {
    // For Read/Write/Edit, try `path` first (Kimi Code convention),
    // then fall back to `file_path` (Claude convention)
    match tool_name {
        "Read" | "Write" | "Edit" => {
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .or_else(|| input.get("file_path").and_then(|v| v.as_str()))
                .unwrap_or("");
            format!("file=\"{path}\"")
        }
        "Glob" => {
            let pattern = input
                .get("pattern")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("pattern=\"{pattern}\"")
        }
        "Bash" | "bash" => {
            let cmd = input
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            let truncated = if cmd.len() > 200 {
                format!("{}...", super::truncate(cmd, 200))
            } else {
                cmd.to_string()
            };
            truncated.replace('\n', " \u{21b5} ")
        }
        _ => format_tool_input(tool_name, input),
    }
}

fn extract_file_path(tool_name: &str, result: &Value) -> Option<String> {
    let args = result.get("args")?;
    match tool_name {
        "Read" | "read" | "Write" | "write" | "Edit" | "edit" => {
            args.get("path").and_then(|v| v.as_str()).map(str::to_string)
        }
        "Glob" | "glob" => {
            args.get("pattern").and_then(|v| v.as_str()).map(str::to_string)
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "kimi_test.rs"]
mod tests;
