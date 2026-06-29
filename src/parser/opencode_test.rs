use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

fn temp_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("cassio-{name}-{unique}"))
}

fn write_json(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

#[test]
fn test_parse_session_orders_messages_and_tracks_stats() {
    let dir = temp_dir("opencode-parse");
    let session_id = "ses_123";
    let message_dir = dir.join("message").join(session_id);

    write_json(
        &dir.join("session/proj_1")
            .join(format!("{session_id}.json")),
        r#"{
            "id": "ses_123",
            "directory": "/workspace/demo",
            "title": "Demo Session",
            "time": { "created": 1704067200000.0 }
        }"#,
    );

    write_json(
        &message_dir.join("msg_assistant_2.json"),
        r#"{
            "id": "msg_assistant_2",
            "role": "assistant",
            "modelID": "model-b",
            "cost": 0.75,
            "time": { "created": 1704067203000.0, "completed": 1704067204000.0 },
            "tokens": { "input": 3, "output": 4, "cache": { "read": 1, "write": 2 } }
        }"#,
    );
    write_json(
        &message_dir.join("msg_user.json"),
        r#"{
            "id": "msg_user",
            "role": "user",
            "time": { "created": 1704067201000.0 }
        }"#,
    );
    write_json(
        &message_dir.join("msg_assistant_1.json"),
        r#"{
            "id": "msg_assistant_1",
            "role": "assistant",
            "modelID": "model-a",
            "cost": 1.25,
            "time": { "created": 1704067202000.0, "completed": 1704067202500.0 },
            "tokens": { "input": 10, "output": 5, "cache": { "read": 2, "write": 3 } }
        }"#,
    );

    write_json(
        &dir.join("part/msg_user/part1.json"),
        r#"{ "type": "text", "text": "  hello from user  " }"#,
    );
    write_json(
        &dir.join("part/msg_user/part2.json"),
        r#"{ "type": "text", "synthetic": true, "text": "skip synthetic" }"#,
    );
    write_json(
        &dir.join("part/msg_user/part3.json"),
        r#"{ "type": "text", "text": "<file>skip context" }"#,
    );
    write_json(
        &dir.join("part/msg_assistant_1/part1.json"),
        r#"{ "type": "text", "text": " first answer " }"#,
    );
    write_json(
        &dir.join("part/msg_assistant_1/part2.json"),
        r#"{
            "type": "tool",
            "tool": "read",
            "state": {
                "title": "Read src/main.rs",
                "input": { "filePath": "src/main.rs" },
                "metadata": { "exit": 0, "description": "Read src/main.rs" }
            }
        }"#,
    );
    write_json(
        &dir.join("part/msg_assistant_2/part1.json"),
        r#"{ "type": "text", "text": " second answer " }"#,
    );
    write_json(
        &dir.join("part/msg_assistant_2/part2.json"),
        r#"{
            "type": "tool",
            "tool": "write",
            "state": {
                "title": "Write src/lib.rs",
                "input": { "filePath": "src/lib.rs" },
                "metadata": { "exit": 1, "description": "Write src/lib.rs" }
            }
        }"#,
    );

    let session = OpenCodeParser.parse_session(&message_dir).unwrap();

    assert_eq!(session.metadata.project_path, "/workspace/demo");
    assert_eq!(session.metadata.title.as_deref(), Some("Demo Session"));
    assert_eq!(session.metadata.model.as_deref(), Some("model-b"));
    assert_eq!(session.stats.user_messages, 1);
    assert_eq!(session.stats.assistant_messages, 2);
    assert_eq!(session.stats.tool_calls, 2);
    assert_eq!(session.stats.tool_errors, 1);
    assert!(session.stats.files_read.contains("src/main.rs"));
    assert!(session.stats.files_written.contains("src/lib.rs"));
    assert_eq!(session.stats.total_tokens.input_tokens, 13);
    assert_eq!(session.stats.total_tokens.output_tokens, 9);
    assert_eq!(session.stats.total_tokens.cache_read_tokens, 3);
    assert_eq!(session.stats.total_tokens.cache_creation_tokens, 5);
    assert_eq!(session.stats.cost, Some(2.0));

    assert_eq!(session.messages.len(), 5);
    assert!(matches!(
        &session.messages[0].content[0],
        ContentBlock::Text { text } if text == "hello from user"
    ));
    assert!(matches!(
        &session.messages[1].content[0],
        ContentBlock::ModelChange { model } if model == "model-a"
    ));
    assert!(
        session.messages[2]
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text } if text == "first answer"))
    );
    assert!(session.messages[2].content.iter().any(|block| matches!(
        block,
        ContentBlock::ToolResult { name, success, .. } if name == "read" && *success
    )));
    assert!(matches!(
        &session.messages[3].content[0],
        ContentBlock::ModelChange { model } if model == "model-b"
    ));
    assert!(session.messages[4].content.iter().any(|block| matches!(
        block,
        ContentBlock::ToolResult { name, success, .. } if name == "write" && !success
    )));

    fs::remove_dir_all(dir).ok();
}

#[test]
fn test_parse_session_without_session_file_uses_fallback_metadata() {
    let dir = temp_dir("opencode-fallback");
    let session_id = "ses_missing";
    let message_dir = dir.join("message").join(session_id);

    write_json(
        &message_dir.join("msg_user.json"),
        r#"{
            "id": "msg_user",
            "role": "user",
            "time": { "created": 1704067201000.0 }
        }"#,
    );
    write_json(
        &dir.join("part/msg_user/part1.json"),
        r#"{ "type": "text", "text": "hello" }"#,
    );

    let session = OpenCodeParser.parse_session(&message_dir).unwrap();

    assert_eq!(session.metadata.session_id, session_id);
    assert!(session.metadata.project_path.is_empty());
    assert_eq!(session.stats.user_messages, 1);

    fs::remove_dir_all(dir).ok();
}
