use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::ast::*;
use crate::error::CassioError;
use crate::parser::Parser;

pub struct CodexParser;

impl Parser for CodexParser {
    fn parse_session(&self, path: &Path) -> Result<Session, CassioError> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        parse_lines(reader.lines().map(|l| l.unwrap_or_default()))
    }
}

impl CodexParser {
    pub fn parse_from_lines<I: Iterator<Item = String>>(lines: I) -> Result<Session, CassioError> {
        parse_lines(lines)
    }
}

#[derive(Deserialize)]
struct CodexRecord {
    timestamp: String,
    #[serde(rename = "type")]
    record_type: String,
    payload: Value,
}

fn parse_lines<I: Iterator<Item = String>>(lines: I) -> Result<Session, CassioError> {
    let mut metadata: Option<SessionMetadata> = None;
    let mut messages: Vec<Message> = Vec::new();
    let mut stats = SessionStats::default();
    let mut current_model: Option<String> = None;
    let mut pending_functions: HashMap<String, (String, String)> = HashMap::new(); // call_id -> (name, args_json)
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
                            let is_error = output.contains("\"exit_code\":") && !output.contains("\"exit_code\":0");
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

fn format_codex_function(name: &str, args_json: &str) -> String {
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
            let truncated = if cmd.len() > 200 { format!("{}...", &cmd[..200]) } else { cmd };
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
                if summary.len() > 150 { format!("{}...", &summary[..150]) } else { summary }
            } else {
                let s = serde_json::to_string(&args).unwrap_or_default();
                if s.len() > 150 { format!("{}...", &s[..150]) } else { s }
            }
        }
        _ => {
            let s = serde_json::to_string(&args).unwrap_or_default();
            if s.len() > 150 { format!("{}...", &s[..150]) } else { s }
        }
    }
}
