use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::ast::*;
use crate::error::CassioError;
use crate::parser::Parser;

pub struct ClaudeParser;

impl Parser for ClaudeParser {
    fn parse_session(&self, path: &Path) -> Result<Session, CassioError> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        parse_lines(reader.lines().map(|l| l.unwrap_or_default()))
    }
}

impl ClaudeParser {
    pub fn parse_from_lines<I: Iterator<Item = String>>(lines: I) -> Result<Session, CassioError> {
        parse_lines(lines)
    }
}

#[derive(Deserialize)]
struct SessionRecord {
    #[serde(rename = "type")]
    record_type: String,
    #[serde(rename = "sessionId")]
    session_id: String,
    timestamp: String,
    cwd: String,
    version: Option<String>,
    #[serde(rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(rename = "isMeta")]
    is_meta: Option<bool>,
    message: Value,
}

fn parse_lines<I: Iterator<Item = String>>(lines: I) -> Result<Session, CassioError> {
    let mut metadata: Option<SessionMetadata> = None;
    let mut messages: Vec<Message> = Vec::new();
    let mut stats = SessionStats::default();
    let mut current_model: Option<String> = None;
    let mut pending_tools: HashMap<String, (String, Value)> = HashMap::new();
    let mut last_timestamp: Option<DateTime<Utc>> = None;
    let mut first_timestamp: Option<DateTime<Utc>> = None;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let record: SessionRecord = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let ts = parse_timestamp(&record.timestamp);
        if let Some(t) = ts {
            if first_timestamp.is_none() {
                first_timestamp = Some(t);
            }
            last_timestamp = Some(t);
        }

        // Capture metadata from first record
        if metadata.is_none() {
            metadata = Some(SessionMetadata {
                session_id: record.session_id.clone(),
                tool: Tool::Claude,
                project_path: record.cwd.clone(),
                started_at: ts.unwrap_or_else(Utc::now),
                version: record.version.clone(),
                git_branch: record.git_branch.clone(),
                model: None,
                title: None,
            });
        }

        match record.record_type.as_str() {
            "user" => {
                if record.is_meta.unwrap_or(false) {
                    continue;
                }
                parse_user_record(
                    &record.message,
                    ts,
                    &mut messages,
                    &mut stats,
                    &mut pending_tools,
                );
            }
            "assistant" => {
                parse_assistant_record(
                    &record.message,
                    ts,
                    &mut messages,
                    &mut stats,
                    &mut current_model,
                    &mut pending_tools,
                );
            }
            "queue-operation" => {
                // Extract content from the raw record
                let raw: Value = serde_json::from_str(trimmed).unwrap_or_default();
                if let Some(content) = raw.get("content").and_then(|c| c.as_str()) {
                    let summary = extract_queue_summary(content);
                    if !summary.is_empty() {
                        messages.push(Message {
                            role: Role::System,
                            timestamp: ts,
                            model: None,
                            content: vec![ContentBlock::QueueOperation { summary }],
                            usage: None,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    let meta = metadata.ok_or_else(|| CassioError::Other("No records found".into()))?;

    // Update model in metadata
    let mut meta = meta;
    meta.model = current_model;

    // Calculate duration
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

fn parse_user_record(
    message: &Value,
    ts: Option<DateTime<Utc>>,
    messages: &mut Vec<Message>,
    stats: &mut SessionStats,
    pending_tools: &mut HashMap<String, (String, Value)>,
) {
    let role = message.get("role").and_then(|r| r.as_str()).unwrap_or("");
    if role != "user" {
        return;
    }

    let content = &message["content"];
    let mut blocks = Vec::new();
    let mut has_user_text = false;

    if let Some(text) = content.as_str() {
        // Skip XML-like system content
        if text.starts_with('<') {
            return;
        }
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            blocks.push(ContentBlock::Text {
                text: trimmed.to_string(),
            });
            has_user_text = true;
        }
    } else if let Some(arr) = content.as_array() {
        for block in arr {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    if text.starts_with('<') {
                        continue;
                    }
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        blocks.push(ContentBlock::Text {
                            text: trimmed.to_string(),
                        });
                        has_user_text = true;
                    }
                }
                "tool_result" => {
                    let tool_use_id = block
                        .get("tool_use_id")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    let is_error = block.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);

                    if let Some((name, input)) = pending_tools.remove(&tool_use_id) {
                        stats.tool_calls += 1;
                        if is_error {
                            stats.tool_errors += 1;
                        }

                        // Track file operations
                        if let Some(file_path) = input.get("file_path").and_then(|f| f.as_str()) {
                            if !is_error {
                                match name.as_str() {
                                    "Read" => {
                                        stats.files_read.insert(file_path.to_string());
                                    }
                                    "Write" => {
                                        stats.files_written.insert(file_path.to_string());
                                    }
                                    "Edit" => {
                                        stats.files_edited.insert(file_path.to_string());
                                    }
                                    _ => {}
                                }
                            }
                        }

                        let summary = format_tool_input(&name, &input);
                        blocks.push(ContentBlock::ToolResult {
                            tool_use_id,
                            name,
                            success: !is_error,
                            summary,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    if has_user_text {
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

fn parse_assistant_record(
    message: &Value,
    ts: Option<DateTime<Utc>>,
    messages: &mut Vec<Message>,
    stats: &mut SessionStats,
    current_model: &mut Option<String>,
    pending_tools: &mut HashMap<String, (String, Value)>,
) {
    let role = message.get("role").and_then(|r| r.as_str()).unwrap_or("");
    if role != "assistant" {
        return;
    }

    let mut blocks = Vec::new();
    let mut has_text = false;

    // Check for model change
    let model = message.get("model").and_then(|m| m.as_str());
    if let Some(m) = model {
        if m != "<synthetic>" && current_model.as_deref() != Some(m) {
            *current_model = Some(m.to_string());
            blocks.push(ContentBlock::ModelChange {
                model: m.to_string(),
            });
        }
    }

    // Track token usage
    if let Some(usage) = message.get("usage") {
        stats.total_tokens.input_tokens +=
            usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        stats.total_tokens.output_tokens +=
            usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        stats.total_tokens.cache_creation_tokens += usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        stats.total_tokens.cache_read_tokens += usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
    }

    let token_usage = message.get("usage").map(|usage| TokenUsage {
        input_tokens: usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        output_tokens: usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        cache_read_tokens: usage
            .get("cache_read_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_creation_tokens: usage
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    });

    if let Some(content_arr) = message.get("content").and_then(|c| c.as_array()) {
        for block in content_arr {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match block_type {
                "text" => {
                    let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
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
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    if !text.is_empty() {
                        blocks.push(ContentBlock::Thinking {
                            text: text.to_string(),
                        });
                    }
                }
                "tool_use" => {
                    let id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or(Value::Object(Default::default()));

                    pending_tools.insert(id.clone(), (name.clone(), input.clone()));

                    blocks.push(ContentBlock::ToolUse {
                        id,
                        name,
                        input,
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
            model: model.map(|m| m.to_string()),
            content: blocks,
            usage: token_usage,
        });
    }
}

fn extract_queue_summary(content: &str) -> String {
    // Extract <summary>...</summary> from queue operation content
    if let Some(start) = content.find("<summary>") {
        let start = start + "<summary>".len();
        if let Some(end) = content[start..].find("</summary>") {
            return content[start..start + end].trim().to_string();
        }
    }
    // Fallback: first 100 chars
    let truncated = if content.len() > 100 {
        &content[..100]
    } else {
        content
    };
    truncated.trim().to_string()
}

pub fn format_tool_input(tool_name: &str, input: &Value) -> String {
    match tool_name {
        "Bash" => {
            let cmd = input
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or("");
            let truncated = if cmd.len() > 200 {
                format!("{}...", &cmd[..200])
            } else {
                cmd.to_string()
            };
            truncated.replace('\n', " \u{21b5} ")
        }
        "Read" => {
            let path = input
                .get("file_path")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            format!("file=\"{path}\"")
        }
        "Write" => {
            let path = input
                .get("file_path")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            format!("file=\"{path}\"")
        }
        "Edit" => {
            let path = input
                .get("file_path")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            format!("file=\"{path}\"")
        }
        "Glob" => {
            let pattern = input
                .get("pattern")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let path = input.get("path").and_then(|p| p.as_str());
            match path {
                Some(p) => format!("pattern=\"{pattern}\" path=\"{p}\""),
                None => format!("pattern=\"{pattern}\""),
            }
        }
        "Grep" => {
            let pattern = input
                .get("pattern")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let path = input.get("path").and_then(|p| p.as_str());
            match path {
                Some(p) => format!("pattern=\"{pattern}\" path=\"{p}\""),
                None => format!("pattern=\"{pattern}\""),
            }
        }
        "Task" => {
            let subagent = input
                .get("subagent_type")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let desc = input
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            format!("{subagent}: \"{desc}\"")
        }
        "WebFetch" => {
            let url = input.get("url").and_then(|u| u.as_str()).unwrap_or("");
            format!("url=\"{url}\"")
        }
        "WebSearch" => {
            let query = input
                .get("query")
                .and_then(|q| q.as_str())
                .unwrap_or("");
            format!("query=\"{query}\"")
        }
        "TodoWrite" => {
            if let Some(todos) = input.get("todos").and_then(|t| t.as_array()) {
                let summary: String = todos
                    .iter()
                    .filter_map(|t| {
                        let content = t.get("content").and_then(|c| c.as_str())?;
                        let status = t.get("status").and_then(|s| s.as_str())?;
                        Some(format!("{status}: {content}"))
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                if summary.len() > 150 {
                    format!("{}...", &summary[..150])
                } else {
                    summary
                }
            } else {
                serde_json::to_string(input).unwrap_or_default()
            }
        }
        _ => {
            let s = serde_json::to_string(input).unwrap_or_default();
            if s.len() > 150 {
                format!("{}...", &s[..150])
            } else {
                s
            }
        }
    }
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    s.parse::<DateTime<Utc>>().ok()
}
