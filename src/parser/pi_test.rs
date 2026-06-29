use super::*;

fn line(value: Value) -> String {
    value.to_string()
}

#[test]
fn test_parse_minimal_pi_session() {
    let lines = vec![
        line(json!({
            "type": "session",
            "version": 3,
            "id": "pi-1",
            "timestamp": "2026-04-13T09:16:32.078Z",
            "cwd": "/proj"
        })),
        line(json!({
            "type": "model_change",
            "timestamp": "2026-04-13T09:16:32.414Z",
            "provider": "openrouter",
            "modelId": "openai/gpt-5.4"
        })),
        line(json!({
            "type": "message",
            "timestamp": "2026-04-13T09:16:37.121Z",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "hi"}],
                "timestamp": 1776071797116_u64
            }
        })),
        line(json!({
            "type": "message",
            "timestamp": "2026-04-13T09:17:13.769Z",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "thinking", "thinking": "internal"},
                    {"type": "text", "text": "hello"}
                ],
                "provider": "openrouter",
                "model": "openai/gpt-5.4",
                "usage": {"input": 10, "output": 5, "cacheRead": 2, "cacheWrite": 1}
            }
        })),
    ];

    let session = PiParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.metadata.tool, Tool::Pi);
    assert_eq!(session.metadata.session_id, "pi-1");
    assert_eq!(session.metadata.project_path, "/proj");
    assert_eq!(
        session.metadata.model.as_deref(),
        Some("openrouter/openai/gpt-5.4")
    );
    assert_eq!(session.stats.user_messages, 1);
    assert_eq!(session.stats.assistant_messages, 1);
    assert_eq!(session.stats.total_tokens.input_tokens, 10);
    assert_eq!(session.messages[0].role, Role::System);
    assert!(matches!(
        session.messages[0].content.first().unwrap(),
        ContentBlock::ModelChange { model } if model == "openrouter/openai/gpt-5.4"
    ));
}

#[test]
fn test_parse_pi_tool_call_and_result_tracks_files() {
    let lines = vec![
        line(json!({
            "type": "session",
            "version": 3,
            "id": "pi-2",
            "timestamp": "2026-04-13T09:45:42.886Z",
            "cwd": "/proj"
        })),
        line(json!({
            "type": "message",
            "timestamp": "2026-04-13T09:46:06.605Z",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "text", "text": "looking"},
                    {"type": "toolCall", "id": "call_1", "name": "read", "arguments": {"path": "src/lib.rs"}}
                ],
                "model": "openai/gpt-5.4",
                "usage": {"input": 100, "output": 20, "cacheRead": 0, "cacheWrite": 0}
            }
        })),
        line(json!({
            "type": "message",
            "timestamp": "2026-04-13T09:46:06.651Z",
            "message": {
                "role": "toolResult",
                "toolCallId": "call_1",
                "toolName": "read",
                "isError": false,
                "content": [{"type": "text", "text": "file contents"}],
                "details": {}
            }
        })),
        line(json!({
            "type": "message",
            "timestamp": "2026-04-13T09:46:07.000Z",
            "message": {
                "role": "assistant",
                "content": [
                    {"type": "toolCall", "id": "call_2", "name": "edit", "arguments": {"path": "src/lib.rs"}}
                ],
                "model": "openai/gpt-5.4",
                "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
            }
        })),
        line(json!({
            "type": "message",
            "timestamp": "2026-04-13T09:46:07.100Z",
            "message": {
                "role": "toolResult",
                "toolCallId": "call_2",
                "toolName": "edit",
                "isError": true,
                "content": [{"type": "text", "text": "boom"}],
                "details": {"code": 1}
            }
        })),
    ];

    let parsed = parse_lines(lines.into_iter(), "stdin".to_string(), None).unwrap();
    let session = parsed.session;
    assert_eq!(session.stats.assistant_messages, 1);
    assert_eq!(session.stats.tool_calls, 2);
    assert_eq!(session.stats.tool_errors, 1);
    assert!(session.stats.files_read.contains("src/lib.rs"));
    assert!(!session.stats.files_edited.contains("src/lib.rs"));

    assert!(session.messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult { name, success, summary, .. }
                if name == "read" && *success && summary == "file=\"src/lib.rs\""
            )
        })
    }));
    assert!(parsed.training.events.iter().any(|event| {
        event.event_kind == "tool_call"
            && event.tool_name.as_deref() == Some("read")
            && event.tool_call_id.as_deref() == Some("call_1")
    }));
}

#[test]
fn test_format_pi_tool_input_variants() {
    assert_eq!(
        format_pi_tool_input("read", &json!({"path": "/tmp/a.rs"})),
        "file=\"/tmp/a.rs\""
    );
    assert_eq!(
        format_pi_tool_input("web_search", &json!({"query": "rust lifetimes"})),
        "query=\"rust lifetimes\""
    );
}
