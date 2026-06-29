use super::*;

fn make_record(record_type: &str, ts: &str, payload: Value) -> String {
    serde_json::json!({
        "type": record_type,
        "timestamp": ts,
        "payload": payload,
    })
    .to_string()
}

fn session_meta(id: &str, cwd: &str) -> Value {
    serde_json::json!({
        "id": id,
        "cwd": cwd,
        "cli_version": "1.0.0",
        "git": {"branch": "main"},
    })
}

fn user_message(text: &str) -> Value {
    serde_json::json!({
        "type": "user_message",
        "message": text,
    })
}

fn assistant_message(text: &str) -> Value {
    serde_json::json!({
        "type": "message",
        "role": "assistant",
        "content": [{"type": "output_text", "text": text}],
    })
}

#[test]
fn test_parse_minimal_codex_session() {
    let lines = vec![
        make_record(
            "session_meta",
            "2025-01-15T10:00:00Z",
            session_meta("s1", "/proj"),
        ),
        make_record("event_msg", "2025-01-15T10:00:01Z", user_message("hello")),
        make_record(
            "response_item",
            "2025-01-15T10:00:02Z",
            assistant_message("hi there"),
        ),
    ];
    let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.metadata.session_id, "s1");
    assert_eq!(session.metadata.tool, Tool::Codex);
    assert_eq!(session.metadata.project_path, "/proj");
    assert_eq!(session.stats.user_messages, 1);
    assert_eq!(session.stats.assistant_messages, 1);
}

#[test]
fn test_parse_function_call_and_output() {
    let lines = vec![
        make_record(
            "session_meta",
            "2025-01-15T10:00:00Z",
            session_meta("s1", "/proj"),
        ),
        make_record(
            "response_item",
            "2025-01-15T10:00:01Z",
            serde_json::json!({
                "type": "function_call",
                "call_id": "c1",
                "name": "shell",
                "arguments": "{\"command\":\"ls\"}",
            }),
        ),
        make_record(
            "response_item",
            "2025-01-15T10:00:02Z",
            serde_json::json!({
                "type": "function_call_output",
                "call_id": "c1",
                "output": "{\"exit_code\":0,\"stdout\":\"files\"}",
            }),
        ),
    ];
    let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.tool_calls, 1);
    assert_eq!(session.stats.tool_errors, 0);
}

#[test]
fn test_parse_function_error() {
    let lines = vec![
        make_record(
            "session_meta",
            "2025-01-15T10:00:00Z",
            session_meta("s1", "/proj"),
        ),
        make_record(
            "response_item",
            "2025-01-15T10:00:01Z",
            serde_json::json!({
                "type": "function_call",
                "call_id": "c1",
                "name": "shell",
                "arguments": "{}",
            }),
        ),
        make_record(
            "response_item",
            "2025-01-15T10:00:02Z",
            serde_json::json!({
                "type": "function_call_output",
                "call_id": "c1",
                "output": "{\"exit_code\":1,\"stderr\":\"error\"}",
            }),
        ),
    ];
    let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.tool_calls, 1);
    assert_eq!(session.stats.tool_errors, 1);
}

#[test]
fn test_parse_model_change_via_turn_context() {
    let lines = vec![
        make_record(
            "session_meta",
            "2025-01-15T10:00:00Z",
            session_meta("s1", "/proj"),
        ),
        make_record(
            "turn_context",
            "2025-01-15T10:00:01Z",
            serde_json::json!({
                "model": "o3-pro",
            }),
        ),
    ];
    let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.metadata.model, Some("o3-pro".to_string()));
    let model_changes: Vec<_> = session
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .filter(|b| matches!(b, ContentBlock::ModelChange { .. }))
        .collect();
    assert_eq!(model_changes.len(), 1);
}

#[test]
fn test_user_message_cleanup_context_blocks() {
    let lines = vec![
        make_record(
            "session_meta",
            "2025-01-15T10:00:00Z",
            session_meta("s1", "/proj"),
        ),
        make_record(
            "event_msg",
            "2025-01-15T10:00:01Z",
            user_message("do this <context ref=\"file.rs\">content</context> please"),
        ),
    ];
    let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
    let user_texts: Vec<_> = session
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| &m.content)
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(user_texts.len(), 1);
    assert!(!user_texts[0].contains("<context"));
    assert!(user_texts[0].contains("do this"));
    assert!(user_texts[0].contains("please"));
}

#[test]
fn test_user_message_cleanup_file_refs() {
    let lines = vec![
        make_record(
            "session_meta",
            "2025-01-15T10:00:00Z",
            session_meta("s1", "/proj"),
        ),
        make_record(
            "event_msg",
            "2025-01-15T10:00:01Z",
            user_message("fix [@main.rs](http://example.com) now"),
        ),
    ];
    let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
    let user_texts: Vec<_> = session
        .messages
        .iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| &m.content)
        .filter_map(|b| {
            if let ContentBlock::Text { text } = b {
                Some(text.as_str())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(user_texts.len(), 1);
    assert!(!user_texts[0].contains("[@"));
}

#[test]
fn test_duration_calculation() {
    let lines = vec![
        make_record(
            "session_meta",
            "2025-01-15T10:00:00Z",
            session_meta("s1", "/proj"),
        ),
        make_record("event_msg", "2025-01-15T10:05:00Z", user_message("hi")),
    ];
    let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.duration_seconds, Some(300));
}

#[test]
fn test_no_session_meta_errors() {
    let lines = vec![make_record(
        "event_msg",
        "2025-01-15T10:00:00Z",
        user_message("hello"),
    )];
    let result = CodexParser::parse_from_lines(lines.into_iter());
    assert!(result.is_err());
}

// --- format_codex_function tests ---

#[test]
fn test_format_codex_function_shell() {
    let result = format_codex_function("shell", r#"{"command":"ls -la"}"#);
    assert_eq!(result, "ls -la");
}

#[test]
fn test_format_codex_function_shell_array() {
    let result = format_codex_function("shell", r#"{"command":["ls","-la"]}"#);
    assert_eq!(result, "ls -la");
}

#[test]
fn test_format_codex_function_read_file() {
    let result = format_codex_function("read_file", r#"{"path":"/foo.rs"}"#);
    assert_eq!(result, "file=\"/foo.rs\"");
}

#[test]
fn test_format_codex_function_write_file() {
    let result = format_codex_function("write_file", r#"{"path":"/bar.rs"}"#);
    assert_eq!(result, "file=\"/bar.rs\"");
}

#[test]
fn test_format_codex_function_update_plan() {
    let result = format_codex_function(
        "update_plan",
        r#"{"plan":[{"step":"do thing","status":"done"},{"step":"next","status":"pending"}]}"#,
    );
    assert!(result.contains("done: do thing"));
    assert!(result.contains("pending: next"));
}

#[test]
fn test_format_codex_function_unknown() {
    let result = format_codex_function("something", r#"{"key":"val"}"#);
    assert!(result.contains("key"));
}

#[test]
fn test_token_count_extraction() {
    let lines = vec![
        make_record(
            "session_meta",
            "2025-01-15T10:00:00Z",
            session_meta("s1", "/proj"),
        ),
        make_record(
            "event_msg",
            "2025-01-15T10:00:01Z",
            serde_json::json!({
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 5402,
                        "cached_input_tokens": 3072,
                        "output_tokens": 237,
                        "reasoning_output_tokens": 192,
                        "total_tokens": 5639
                    }
                }
            }),
        ),
        // A later token_count should overwrite (cumulative)
        make_record(
            "event_msg",
            "2025-01-15T10:00:05Z",
            serde_json::json!({
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 10000,
                        "cached_input_tokens": 6000,
                        "output_tokens": 500,
                        "reasoning_output_tokens": 300,
                        "total_tokens": 16800
                    }
                }
            }),
        ),
    ];
    let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.total_tokens.input_tokens, 10000);
    assert_eq!(session.stats.total_tokens.cache_read_tokens, 6000);
    assert_eq!(session.stats.total_tokens.output_tokens, 800); // 500 + 300 reasoning
}

#[test]
fn test_token_count_null_info() {
    let lines = vec![
        make_record(
            "session_meta",
            "2025-01-15T10:00:00Z",
            session_meta("s1", "/proj"),
        ),
        make_record(
            "event_msg",
            "2025-01-15T10:00:01Z",
            serde_json::json!({
                "type": "token_count",
                "info": null,
                "rate_limits": {}
            }),
        ),
    ];
    let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.total_tokens.input_tokens, 0);
    assert_eq!(session.stats.total_tokens.output_tokens, 0);
}

#[test]
fn test_file_read_tracking_from_shell() {
    let lines = vec![
        make_record(
            "session_meta",
            "2025-01-15T10:00:00Z",
            session_meta("s1", "/proj"),
        ),
        make_record(
            "response_item",
            "2025-01-15T10:00:01Z",
            serde_json::json!({
                "type": "function_call",
                "call_id": "c1",
                "name": "shell",
                "arguments": "{\"command\":\"cat /foo/bar.rs\"}",
            }),
        ),
        make_record(
            "response_item",
            "2025-01-15T10:00:02Z",
            serde_json::json!({
                "type": "function_call_output",
                "call_id": "c1",
                "output": "{\"exit_code\":0}",
            }),
        ),
    ];
    let session = CodexParser::parse_from_lines(lines.into_iter()).unwrap();
    assert!(session.stats.files_read.contains("/foo/bar.rs"));
}
