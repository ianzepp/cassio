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

fn response_user_message(text: &str) -> Value {
    serde_json::json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": text}],
    })
}

fn parse_user_records(records: Vec<(&str, Value)>) -> ParsedSession {
    let mut lines = vec![make_record(
        "session_meta",
        "2026-10-26T10:00:00Z",
        session_meta("user-repro", "/proj"),
    )];
    for (i, (kind, payload)) in records.into_iter().enumerate() {
        lines.push(make_record(
            kind,
            &format!("2026-10-26T10:00:{:02}Z", i + 1),
            payload,
        ));
    }
    parse_lines(lines.into_iter(), "stdin".into(), None).unwrap()
}

fn exported_user_texts(parsed: &ParsedSession) -> Vec<&str> {
    parsed
        .session
        .messages
        .iter()
        .filter(|message| message.role == Role::User)
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn test_response_user_text_reaches_transcript_and_training_export() {
    use crate::formatter::Formatter;

    let parsed = parse_user_records(vec![
        (
            "response_item",
            response_user_message("Please fix the bug."),
        ),
        ("response_item", assistant_message("I will.")),
    ]);
    assert_eq!(exported_user_texts(&parsed), ["Please fix the bug."]);
    assert_eq!(parsed.session.stats.user_messages, 1);
    assert_eq!(parsed.training.stats.user_messages, 1);
    let event = parsed
        .training
        .events
        .iter()
        .find(|e| e.role.as_deref() == Some("user"))
        .unwrap();
    assert_eq!(event.raw_text.as_deref(), Some("Please fix the bug."));
    assert_eq!(event.source_record_refs, ["jsonl:2"]);
    assert_eq!(event.timestamp, parsed.session.messages[0].timestamp);
    let mut output = Vec::new();
    crate::formatter::emoji_text::EmojiTextFormatter
        .format(&parsed, &mut output)
        .unwrap();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("👤 Please fix the bug."));
    assert!(text.contains("Messages: 1 user, 1 assistant"));
}

#[test]
fn test_user_copies_deduplicated_in_either_order_with_both_source_refs() {
    for response_first in [true, false] {
        let mut records = vec![
            ("response_item", response_user_message("hello")),
            ("event_msg", user_message("hello")),
        ];
        if !response_first {
            records.reverse();
        }
        let parsed = parse_user_records(records);
        assert_eq!(exported_user_texts(&parsed), ["hello"]);
        assert_eq!(parsed.session.stats.user_messages, 1);
        assert_eq!(parsed.training.stats.user_messages, 1);
        assert_eq!(parsed.training.events.len(), 1);
        assert_eq!(
            parsed.training.events[0].source_record_refs,
            ["jsonl:2", "jsonl:3"]
        );
    }
}

#[test]
fn test_repeated_user_prompts_survive_across_turns_and_formats() {
    let parsed = parse_user_records(vec![
        ("event_msg", user_message("again")),
        ("response_item", assistant_message("first")),
        ("response_item", response_user_message("again")),
        ("event_msg", user_message("again")),
        ("response_item", assistant_message("second")),
        ("response_item", response_user_message("again")),
    ]);
    assert_eq!(exported_user_texts(&parsed), ["again", "again", "again"]);
    assert_eq!(parsed.session.stats.user_messages, 3);
    assert_eq!(parsed.training.stats.user_messages, 3);
}

#[test]
fn test_repeated_same_source_messages_are_not_deduplicated() {
    let parsed = parse_user_records(vec![
        ("response_item", response_user_message("again")),
        ("response_item", response_user_message("again")),
        ("event_msg", user_message("again")),
        ("event_msg", user_message("again")),
    ]);
    assert_eq!(exported_user_texts(&parsed), ["again", "again"]);
    assert_eq!(parsed.training.events.len(), 2);
}

#[test]
fn test_response_user_content_blocks_exclude_injected_context() {
    let parsed = parse_user_records(vec![
        (
            "response_item",
            response_user_message("# AGENTS.md instructions\n\n<INSTRUCTIONS>rules</INSTRUCTIONS>"),
        ),
        (
            "response_item",
            response_user_message("<environment_context>cwd</environment_context>"),
        ),
        (
            "response_item",
            response_user_message("<subagent_notification>completed</subagent_notification>"),
        ),
        (
            "response_item",
            serde_json::json!({
                "type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "<environment_context>cwd</environment_context>"},
                    {"type": "input_text", "text": "Look at this image."},
                    {"type": "input_image", "image_url": "data:image/png;base64,ignored"},
                    {"type": "input_text", "text": "Then fix <context ref=\"a.rs\">code</context> [@a.rs](file) please."}
                ],
            }),
        ),
    ]);
    assert_eq!(
        exported_user_texts(&parsed),
        ["Look at this image.\nThen fix  please."]
    );
    assert_eq!(parsed.session.stats.user_messages, 1);
    assert_eq!(parsed.training.events.len(), 1);
    assert!(
        parsed.training.events[0]
            .raw_text
            .as_deref()
            .unwrap()
            .contains("<context ref=")
    );
}

#[test]
fn test_user_xml_is_preserved_unless_it_is_known_injected_context() {
    let parsed = parse_user_records(vec![(
        "response_item",
        response_user_message("<example>keep this</example>"),
    )]);
    assert_eq!(
        exported_user_texts(&parsed),
        ["<example>keep this</example>"]
    );
}

#[test]
fn test_task_boundaries_keep_identical_prompts_without_assistant_text() {
    for boundary in ["task_started", "task_complete"] {
        let parsed = parse_user_records(vec![
            ("event_msg", user_message("again")),
            ("event_msg", serde_json::json!({"type": boundary})),
            ("response_item", response_user_message("again")),
        ]);
        assert_eq!(exported_user_texts(&parsed), ["again", "again"]);
        assert_eq!(parsed.training.events.len(), 2);
    }
}

#[test]
fn test_distinct_user_text_from_each_source_is_preserved() {
    let parsed = parse_user_records(vec![
        ("event_msg", user_message("first")),
        ("response_item", response_user_message("second")),
    ]);
    assert_eq!(exported_user_texts(&parsed), ["first", "second"]);
    assert_eq!(parsed.training.events.len(), 2);
}
