use super::*;
use serde_json::json;

fn line(value: serde_json::Value) -> String {
    value.to_string()
}

#[test]
fn test_parse_minimal_cursor_session() {
    let lines = vec![
        line(json!({
            "role": "user",
            "message": {"content": [{"type": "text", "text": "warm up please"}]}
        })),
        line(json!({
            "role": "assistant",
            "message": {"content": [{"type": "text", "text": "Sure."}]}
        })),
    ];

    let session = CursorParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.metadata.tool, Tool::Cursor);
    assert_eq!(session.stats.user_messages, 1);
    assert_eq!(session.stats.assistant_messages, 1);
}

#[test]
fn test_parse_cursor_tool_use_blocks() {
    let lines = vec![
        line(json!({
            "role": "assistant",
            "message": {"content": [
                {"type": "text", "text": "Reading file"},
                {"type": "tool_use", "name": "Read", "input": {"path": "src/lib.rs"}}
            ]}
        })),
    ];

    let session = CursorParser::parse_from_lines(lines.into_iter()).unwrap();
    assert_eq!(session.stats.tool_calls, 1);
    assert!(session.stats.files_read.contains("src/lib.rs"));
    assert!(session.messages.iter().any(|msg| {
        msg.content.iter().any(|block| matches!(block, ContentBlock::ToolUse { name, .. } if name == "Read"))
    }));
}

#[test]
fn test_cursor_project_path_from_source() {
    let path = Path::new(
        "/Users/me/.cursor/projects/Users-ianzepp-work-ianzepp-cassio/agent-transcripts/abc/abc.jsonl",
    );
    assert_eq!(
        cursor_project_path_from_source(path).as_deref(),
        Some("/Users/ianzepp/work/ianzepp/cassio")
    );
}