use super::*;
use serde_json::json;

fn line(value: serde_json::Value) -> String {
    value.to_string()
}

#[test]
fn test_parse_minimal_grok_session() {
    let lines = vec![
        line(json!({
            "type": "user",
            "content": [{"type": "text", "text": "<user_query>\nwarm up please\n</user_query>"}]
        })),
        line(json!({
            "type": "assistant",
            "content": "On it.",
            "model_id": "grok-composer-2.5-fast"
        })),
    ];

    let session = GrokParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.metadata.tool, Tool::Grok);
    assert_eq!(session.metadata.session_id, "stdin");
    assert_eq!(session.stats.user_messages, 1);
    assert_eq!(session.stats.assistant_messages, 1);
    assert_eq!(
        session.metadata.model.as_deref(),
        Some("grok-composer-2.5-fast")
    );
}

#[test]
fn test_parse_grok_tool_call_and_result() {
    let lines = vec![
        line(json!({
            "type": "assistant",
            "content": "",
            "tool_calls": [{
                "id": "call-1",
                "name": "Read",
                "arguments": "{\"path\":\"src/lib.rs\"}"
            }],
            "model_id": "grok-composer-2.5-fast"
        })),
        line(json!({
            "type": "tool_result",
            "tool_call_id": "call-1",
            "content": "fn main() {}\n"
        })),
    ];

    let session = GrokParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.tool_calls, 1);
    assert_eq!(session.stats.tool_errors, 0);
    assert!(session.stats.files_read.contains("src/lib.rs"));
    assert!(session.messages.iter().any(|msg| {
        msg.content.iter().any(|block| {
            matches!(
                block,
                ContentBlock::ToolResult {
                    name,
                    success,
                    summary,
                    ..
                } if name == "Read" && *success && summary == "file=\"src/lib.rs\""
            )
        })
    }));
}

#[test]
fn test_grok_tool_result_detects_shell_failure() {
    assert!(!grok_tool_result_failed("Exit code: 0\n"));
    assert!(grok_tool_result_failed("Exit code: 1\n"));
}

#[test]
fn test_extract_grok_user_text_skips_context_injection() {
    let record = json!({
        "type": "user",
        "content": [{"type": "text", "text": "<user_info>\nOS: darwin\n</user_info>"}]
    });
    assert!(extract_grok_user_text(&record).is_none());
}