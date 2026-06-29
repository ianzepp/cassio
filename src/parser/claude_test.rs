use super::*;

fn make_session_record(
    session_id: &str,
    ts: &str,
    cwd: &str,
    record_type: &str,
    message: Value,
) -> String {
    serde_json::json!({
        "type": record_type,
        "sessionId": session_id,
        "timestamp": ts,
        "cwd": cwd,
        "message": message,
    })
    .to_string()
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
        make_session_record(
            "ses1",
            "2025-01-15T10:00:00Z",
            "/proj",
            "user",
            make_user_text("hello"),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:05Z",
            "/proj",
            "assistant",
            make_assistant(
                vec![serde_json::json!({"type": "text", "text": "Hi there!"})],
                Some("claude-sonnet-4-5-20250929"),
                None,
            ),
        ),
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
        make_session_record(
            "ses1",
            "2025-01-15T10:00:00Z",
            "/proj",
            "user",
            make_user_text("read a file"),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:01Z",
            "/proj",
            "assistant",
            make_assistant(
                vec![serde_json::json!({
                    "type": "tool_use",
                    "id": "tool1",
                    "name": "Read",
                    "input": {"file_path": "/foo/bar.rs"},
                })],
                Some("claude-sonnet-4-5-20250929"),
                None,
            ),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:02Z",
            "/proj",
            "user",
            make_user_array(vec![serde_json::json!({
                "type": "tool_result",
                "tool_use_id": "tool1",
                "is_error": false,
            })]),
        ),
    ];
    let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.tool_calls, 1);
    assert_eq!(session.stats.tool_errors, 0);
    assert!(session.stats.files_read.contains("/foo/bar.rs"));
}

#[test]
fn test_parse_tool_error() {
    let lines = vec![
        make_session_record(
            "ses1",
            "2025-01-15T10:00:00Z",
            "/proj",
            "user",
            make_user_text("do something"),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:01Z",
            "/proj",
            "assistant",
            make_assistant(
                vec![serde_json::json!({
                    "type": "tool_use",
                    "id": "tool1",
                    "name": "Bash",
                    "input": {"command": "ls"},
                })],
                None,
                None,
            ),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:02Z",
            "/proj",
            "user",
            make_user_array(vec![serde_json::json!({
                "type": "tool_result",
                "tool_use_id": "tool1",
                "is_error": true,
            })]),
        ),
    ];
    let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.tool_calls, 1);
    assert_eq!(session.stats.tool_errors, 1);
}

#[test]
fn test_parse_file_write_and_edit_tracking() {
    let lines = vec![
        make_session_record(
            "ses1",
            "2025-01-15T10:00:00Z",
            "/proj",
            "user",
            make_user_text("write a file"),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:01Z",
            "/proj",
            "assistant",
            make_assistant(
                vec![
                    serde_json::json!({"type": "tool_use", "id": "t1", "name": "Write", "input": {"file_path": "/a.rs"}}),
                    serde_json::json!({"type": "tool_use", "id": "t2", "name": "Edit", "input": {"file_path": "/b.rs"}}),
                ],
                None,
                None,
            ),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:02Z",
            "/proj",
            "user",
            make_user_array(vec![
                serde_json::json!({"type": "tool_result", "tool_use_id": "t1", "is_error": false}),
                serde_json::json!({"type": "tool_result", "tool_use_id": "t2", "is_error": false}),
            ]),
        ),
    ];
    let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
    assert!(session.stats.files_written.contains("/a.rs"));
    assert!(session.stats.files_edited.contains("/b.rs"));
}

#[test]
fn test_parse_model_change() {
    let lines = vec![
        make_session_record(
            "ses1",
            "2025-01-15T10:00:00Z",
            "/proj",
            "user",
            make_user_text("hi"),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:01Z",
            "/proj",
            "assistant",
            make_assistant(
                vec![serde_json::json!({"type": "text", "text": "hello"})],
                Some("claude-sonnet-4-5-20250929"),
                None,
            ),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:02Z",
            "/proj",
            "assistant",
            make_assistant(
                vec![serde_json::json!({"type": "text", "text": "switching"})],
                Some("claude-opus-4-5-20251101"),
                None,
            ),
        ),
    ];
    let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
    // Should have ModelChange blocks
    let model_changes: Vec<_> = session
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, ContentBlock::ModelChange { .. }))
        .collect();
    assert_eq!(model_changes.len(), 2); // initial + change
    assert_eq!(
        session.metadata.model,
        Some("claude-opus-4-5-20251101".to_string())
    );
}

#[test]
fn test_parse_token_usage() {
    let lines = vec![
        make_session_record(
            "ses1",
            "2025-01-15T10:00:00Z",
            "/proj",
            "user",
            make_user_text("hi"),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:01Z",
            "/proj",
            "assistant",
            make_assistant(
                vec![serde_json::json!({"type": "text", "text": "hello"})],
                None,
                Some(serde_json::json!({
                    "input_tokens": 100,
                    "output_tokens": 50,
                    "cache_read_input_tokens": 20,
                    "cache_creation_input_tokens": 10,
                })),
            ),
        ),
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
        make_session_record(
            "ses1",
            "2025-01-15T10:00:00Z",
            "/proj",
            "user",
            make_user_text("hello"),
        ),
        record.to_string(),
    ];
    let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.user_messages, 1); // only the non-meta one
}

#[test]
fn test_skip_xml_content() {
    let lines = vec![
        make_session_record(
            "ses1",
            "2025-01-15T10:00:00Z",
            "/proj",
            "user",
            make_user_text("<system>some xml</system>"),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:01Z",
            "/proj",
            "assistant",
            make_assistant(
                vec![serde_json::json!({"type": "text", "text": "ok"})],
                None,
                None,
            ),
        ),
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
        make_session_record(
            "ses1",
            "2025-01-15T10:00:00Z",
            "/proj",
            "user",
            make_user_text("hello"),
        ),
        record.to_string(),
    ];
    let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
    let queue_ops: Vec<_> = session
        .messages
        .iter()
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
        make_session_record(
            "ses1",
            "2025-01-15T10:00:00Z",
            "/proj",
            "user",
            make_user_text("think about this"),
        ),
        make_session_record(
            "ses1",
            "2025-01-15T10:00:01Z",
            "/proj",
            "assistant",
            make_assistant(
                vec![
                    serde_json::json!({"type": "thinking", "thinking": "let me think..."}),
                    serde_json::json!({"type": "text", "text": "here is my answer"}),
                ],
                None,
                None,
            ),
        ),
    ];
    let session = ClaudeParser::parse_from_lines(lines.into_iter()).unwrap();
    let thinking: Vec<_> = session
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, ContentBlock::Thinking { .. }))
        .collect();
    assert_eq!(thinking.len(), 1);
}

// --- extract_queue_summary tests ---

#[test]
fn test_extract_queue_summary_with_tags() {
    assert_eq!(
        extract_queue_summary("before <summary>the summary</summary> after"),
        "the summary"
    );
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
    assert_eq!(
        format_tool_input("Glob", &input),
        "pattern=\"*.rs\" path=\"/src\""
    );
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
    assert_eq!(
        format_tool_input("WebFetch", &input),
        "url=\"https://example.com\""
    );
}

#[test]
fn test_format_tool_input_websearch() {
    let input = serde_json::json!({"query": "rust testing"});
    assert_eq!(
        format_tool_input("WebSearch", &input),
        "query=\"rust testing\""
    );
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
