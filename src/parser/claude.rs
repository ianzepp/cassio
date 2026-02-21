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
        super::truncate(content, 100)
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
                format!("{}...", super::truncate(cmd, 200))
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
                    format!("{}...", super::truncate(&summary, 150))
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
                format!("{}...", super::truncate(&s, 150))
            } else {
                s
            }
        }
    }
}

fn parse_timestamp(s: &str) -> Option<DateTime<Utc>> {
    s.parse::<DateTime<Utc>>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session_record(session_id: &str, ts: &str, cwd: &str, record_type: &str, message: Value) -> String {
        serde_json::json!({
            "type": record_type,
            "sessionId": session_id,
            "timestamp": ts,
            "cwd": cwd,
            "message": message,
        }).to_string()
    }

    fn make_user_text(text: &str) -> Value {
        serde_json::json!({
            "role": "user",
            "content": text,
        })
    }

    fn make_user_array(blocks: Vec<Value>) -> Value {
        serde_json::json!({
            "role": "user",
            "content": blocks,
        })
    }

    fn make_assistant(content: Vec<Value>, model: Option<&str>, usage: Option<Value>) -> Value {
        let mut msg = serde_json::json!({
            "role": "assistant",
            "content": content,
        });
        if let Some(m) = model {
            msg["model"] = serde_json::json!(m);
        }
        if let Some(u) = usage {
            msg["usage"] = u;
        }
        msg
    }

    #[test]
    fn test_parse_minimal_session() {
        let lines = vec![
            make_session_record("ses1", "2025-01-15T10:00:00Z", "/proj", "user",
                make_user_text("hello")),
            make_session_record("ses1", "2025-01-15T10:00:05Z", "/proj", "assistant",
                make_assistant(
                    vec![serde_json::json!({"type": "text", "text": "Hi there!"})],
                    Some("claude-sonnet-4-5-20250929"),
                    None,
                )),
        ];
        let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.metadata.session_id, "ses1");
        assert_eq!(session.metadata.tool, Tool::Claude);
        assert_eq!(session.metadata.project_path, "/proj");
        assert_eq!(session.stats.user_messages, 1);
        assert_eq!(session.stats.assistant_messages, 1);
        assert_eq!(session.stats.duration_seconds, Some(5));
    }

    #[test]
    fn test_parse_tool_use_and_result() {
        let lines = vec![
            make_session_record("ses1", "2025-01-15T10:00:00Z", "/proj", "user",
                make_user_text("read a file")),
            make_session_record("ses1", "2025-01-15T10:00:01Z", "/proj", "assistant",
                make_assistant(
                    vec![serde_json::json!({
                        "type": "tool_use",
                        "id": "tool1",
                        "name": "Read",
                        "input": {"file_path": "/foo/bar.rs"},
                    })],
                    Some("claude-sonnet-4-5-20250929"),
                    None,
                )),
            make_session_record("ses1", "2025-01-15T10:00:02Z", "/proj", "user",
                make_user_array(vec![serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": "tool1",
                    "is_error": false,
                })])),
        ];
        let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.stats.tool_calls, 1);
        assert_eq!(session.stats.tool_errors, 0);
        assert!(session.stats.files_read.contains("/foo/bar.rs"));
    }

    #[test]
    fn test_parse_tool_error() {
        let lines = vec![
            make_session_record("ses1", "2025-01-15T10:00:00Z", "/proj", "user",
                make_user_text("do something")),
            make_session_record("ses1", "2025-01-15T10:00:01Z", "/proj", "assistant",
                make_assistant(
                    vec![serde_json::json!({
                        "type": "tool_use",
                        "id": "tool1",
                        "name": "Bash",
                        "input": {"command": "ls"},
                    })],
                    None,
                    None,
                )),
            make_session_record("ses1", "2025-01-15T10:00:02Z", "/proj", "user",
                make_user_array(vec![serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": "tool1",
                    "is_error": true,
                })])),
        ];
        let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.stats.tool_calls, 1);
        assert_eq!(session.stats.tool_errors, 1);
    }

    #[test]
    fn test_parse_file_write_and_edit_tracking() {
        let lines = vec![
            make_session_record("ses1", "2025-01-15T10:00:00Z", "/proj", "user",
                make_user_text("write a file")),
            make_session_record("ses1", "2025-01-15T10:00:01Z", "/proj", "assistant",
                make_assistant(
                    vec![
                        serde_json::json!({"type": "tool_use", "id": "t1", "name": "Write", "input": {"file_path": "/a.rs"}}),
                        serde_json::json!({"type": "tool_use", "id": "t2", "name": "Edit", "input": {"file_path": "/b.rs"}}),
                    ],
                    None, None,
                )),
            make_session_record("ses1", "2025-01-15T10:00:02Z", "/proj", "user",
                make_user_array(vec![
                    serde_json::json!({"type": "tool_result", "tool_use_id": "t1", "is_error": false}),
                    serde_json::json!({"type": "tool_result", "tool_use_id": "t2", "is_error": false}),
                ])),
        ];
        let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
        assert!(session.stats.files_written.contains("/a.rs"));
        assert!(session.stats.files_edited.contains("/b.rs"));
    }

    #[test]
    fn test_parse_model_change() {
        let lines = vec![
            make_session_record("ses1", "2025-01-15T10:00:00Z", "/proj", "user",
                make_user_text("hi")),
            make_session_record("ses1", "2025-01-15T10:00:01Z", "/proj", "assistant",
                make_assistant(
                    vec![serde_json::json!({"type": "text", "text": "hello"})],
                    Some("claude-sonnet-4-5-20250929"),
                    None,
                )),
            make_session_record("ses1", "2025-01-15T10:00:02Z", "/proj", "assistant",
                make_assistant(
                    vec![serde_json::json!({"type": "text", "text": "switching"})],
                    Some("claude-opus-4-5-20251101"),
                    None,
                )),
        ];
        let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
        // Should have ModelChange blocks
        let model_changes: Vec<_> = session.messages.iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::ModelChange { .. }))
            .collect();
        assert_eq!(model_changes.len(), 2); // initial + change
        assert_eq!(session.metadata.model, Some("claude-opus-4-5-20251101".to_string()));
    }

    #[test]
    fn test_parse_token_usage() {
        let lines = vec![
            make_session_record("ses1", "2025-01-15T10:00:00Z", "/proj", "user",
                make_user_text("hi")),
            make_session_record("ses1", "2025-01-15T10:00:01Z", "/proj", "assistant",
                make_assistant(
                    vec![serde_json::json!({"type": "text", "text": "hello"})],
                    None,
                    Some(serde_json::json!({
                        "input_tokens": 100,
                        "output_tokens": 50,
                        "cache_read_input_tokens": 20,
                        "cache_creation_input_tokens": 10,
                    })),
                )),
        ];
        let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.stats.total_tokens.input_tokens, 100);
        assert_eq!(session.stats.total_tokens.output_tokens, 50);
        assert_eq!(session.stats.total_tokens.cache_read_tokens, 20);
        assert_eq!(session.stats.total_tokens.cache_creation_tokens, 10);
    }

    #[test]
    fn test_skip_is_meta_records() {
        let record = serde_json::json!({
            "type": "user",
            "sessionId": "ses1",
            "timestamp": "2025-01-15T10:00:00Z",
            "cwd": "/proj",
            "isMeta": true,
            "message": {"role": "user", "content": "meta stuff"},
        });
        let lines = vec![
            // Normal first record to establish metadata
            make_session_record("ses1", "2025-01-15T10:00:00Z", "/proj", "user",
                make_user_text("hello")),
            record.to_string(),
        ];
        let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.stats.user_messages, 1); // only the non-meta one
    }

    #[test]
    fn test_skip_xml_content() {
        let lines = vec![
            make_session_record("ses1", "2025-01-15T10:00:00Z", "/proj", "user",
                make_user_text("<system>some xml</system>")),
            make_session_record("ses1", "2025-01-15T10:00:01Z", "/proj", "assistant",
                make_assistant(
                    vec![serde_json::json!({"type": "text", "text": "ok"})],
                    None, None,
                )),
        ];
        let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
        assert_eq!(session.stats.user_messages, 0); // XML content skipped
    }

    #[test]
    fn test_parse_queue_operation() {
        let record = serde_json::json!({
            "type": "queue-operation",
            "sessionId": "ses1",
            "timestamp": "2025-01-15T10:00:00Z",
            "cwd": "/proj",
            "content": "stuff <summary>queued task description</summary> more",
            "message": {},
        });
        let lines = vec![
            make_session_record("ses1", "2025-01-15T10:00:00Z", "/proj", "user",
                make_user_text("hello")),
            record.to_string(),
        ];
        let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
        let queue_ops: Vec<_> = session.messages.iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::QueueOperation { .. }))
            .collect();
        assert_eq!(queue_ops.len(), 1);
        if let ContentBlock::QueueOperation { summary } = &queue_ops[0] {
            assert_eq!(summary, "queued task description");
        }
    }

    #[test]
    fn test_parse_thinking_block() {
        let lines = vec![
            make_session_record("ses1", "2025-01-15T10:00:00Z", "/proj", "user",
                make_user_text("think about this")),
            make_session_record("ses1", "2025-01-15T10:00:01Z", "/proj", "assistant",
                make_assistant(
                    vec![
                        serde_json::json!({"type": "thinking", "thinking": "let me think..."}),
                        serde_json::json!({"type": "text", "text": "here is my answer"}),
                    ],
                    None, None,
                )),
        ];
        let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
        let thinking: Vec<_> = session.messages.iter()
            .flat_map(|m| &m.content)
            .filter(|b| matches!(b, ContentBlock::Thinking { .. }))
            .collect();
        assert_eq!(thinking.len(), 1);
    }

    // --- extract_queue_summary tests ---

    #[test]
    fn test_extract_queue_summary_with_tags() {
        assert_eq!(extract_queue_summary("before <summary>the summary</summary> after"), "the summary");
    }

    #[test]
    fn test_extract_queue_summary_without_tags() {
        assert_eq!(extract_queue_summary("just plain text"), "just plain text");
    }

    #[test]
    fn test_extract_queue_summary_truncation() {
        let long = "a".repeat(200);
        let result = extract_queue_summary(&long);
        assert_eq!(result.len(), 100);
    }

    // --- format_tool_input tests ---

    #[test]
    fn test_format_tool_input_bash() {
        let input = serde_json::json!({"command": "ls -la\nfoo"});
        let result = format_tool_input("Bash", &input);
        assert!(result.contains("ls -la"));
        assert!(result.contains("\u{21b5}")); // newline replaced
    }

    #[test]
    fn test_format_tool_input_read() {
        let input = serde_json::json!({"file_path": "/foo/bar.rs"});
        assert_eq!(format_tool_input("Read", &input), "file=\"/foo/bar.rs\"");
    }

    #[test]
    fn test_format_tool_input_write() {
        let input = serde_json::json!({"file_path": "/foo/bar.rs"});
        assert_eq!(format_tool_input("Write", &input), "file=\"/foo/bar.rs\"");
    }

    #[test]
    fn test_format_tool_input_edit() {
        let input = serde_json::json!({"file_path": "/foo/bar.rs"});
        assert_eq!(format_tool_input("Edit", &input), "file=\"/foo/bar.rs\"");
    }

    #[test]
    fn test_format_tool_input_glob() {
        let input = serde_json::json!({"pattern": "*.rs", "path": "/src"});
        assert_eq!(format_tool_input("Glob", &input), "pattern=\"*.rs\" path=\"/src\"");
    }

    #[test]
    fn test_format_tool_input_glob_no_path() {
        let input = serde_json::json!({"pattern": "*.rs"});
        assert_eq!(format_tool_input("Glob", &input), "pattern=\"*.rs\"");
    }

    #[test]
    fn test_format_tool_input_grep() {
        let input = serde_json::json!({"pattern": "TODO"});
        assert_eq!(format_tool_input("Grep", &input), "pattern=\"TODO\"");
    }

    #[test]
    fn test_format_tool_input_task() {
        let input = serde_json::json!({"subagent_type": "Explore", "description": "find files"});
        assert_eq!(format_tool_input("Task", &input), "Explore: \"find files\"");
    }

    #[test]
    fn test_format_tool_input_webfetch() {
        let input = serde_json::json!({"url": "https://example.com"});
        assert_eq!(format_tool_input("WebFetch", &input), "url=\"https://example.com\"");
    }

    #[test]
    fn test_format_tool_input_websearch() {
        let input = serde_json::json!({"query": "rust testing"});
        assert_eq!(format_tool_input("WebSearch", &input), "query=\"rust testing\"");
    }

    #[test]
    fn test_format_tool_input_unknown() {
        let input = serde_json::json!({"key": "value"});
        let result = format_tool_input("UnknownTool", &input);
        assert!(result.contains("key"));
    }

    #[test]
    fn test_format_tool_input_bash_long_truncation() {
        let long_cmd = "x".repeat(300);
        let input = serde_json::json!({"command": long_cmd});
        let result = format_tool_input("Bash", &input);
        assert!(result.len() <= 203); // 200 + "..."
    }

    #[test]
    fn test_format_tool_input_todowrite() {
        let input = serde_json::json!({
            "todos": [
                {"content": "fix bug", "status": "pending"},
                {"content": "add test", "status": "done"},
            ]
        });
        let result = format_tool_input("TodoWrite", &input);
        assert!(result.contains("pending: fix bug"));
        assert!(result.contains("done: add test"));
    }

    // --- parse_timestamp ---

    #[test]
    fn test_parse_timestamp_valid() {
        assert!(parse_timestamp("2025-01-15T10:00:00Z").is_some());
    }

    #[test]
    fn test_parse_timestamp_invalid() {
        assert!(parse_timestamp("not a timestamp").is_none());
    }

    #[test]
    fn test_empty_input_errors() {
        let lines: Vec<String> = vec![];
        let result = ClaudeParser::parse_from_lines(lines.into_iter());
        assert!(result.is_err());
    }
}
