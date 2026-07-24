use super::*;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("cassio-claude-chat-{name}-{nanos}"))
}

fn sample_conversations() -> Value {
    json!([
        {
            "uuid": "aaa-111",
            "name": "Canned tuna storage requirements",
            "summary": "",
            "created_at": "2026-07-20T12:00:00.000000Z",
            "updated_at": "2026-07-20T12:05:00.000000Z",
            "account": {"uuid": "acct"},
            "chat_messages": [
                {
                    "uuid": "m1",
                    "text": "How long can canned tuna last?",
                    "content": [
                        {
                            "type": "text",
                            "text": "How long can canned tuna last?"
                        }
                    ],
                    "sender": "human",
                    "created_at": "2026-07-20T12:00:01.000000Z",
                    "updated_at": "2026-07-20T12:00:01.000000Z",
                    "attachments": [],
                    "files": []
                },
                {
                    "uuid": "m2",
                    "text": "Unopened, years past the date.",
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "Food safety question."
                        },
                        {
                            "type": "text",
                            "text": "Unopened, years past the date."
                        }
                    ],
                    "sender": "assistant",
                    "created_at": "2026-07-20T12:00:10.000000Z",
                    "updated_at": "2026-07-20T12:00:10.000000Z",
                    "attachments": [],
                    "files": []
                }
            ]
        },
        {
            "uuid": "bbb-222",
            "name": "",
            "summary": null,
            "created_at": "2026-07-21T08:00:00Z",
            "updated_at": "2026-07-21T08:01:00Z",
            "account": {"uuid": "acct"},
            "chat_messages": [
                {
                    "uuid": "m3",
                    "text": "hi",
                    "content": [{"type": "text", "text": "hi"}],
                    "sender": "human",
                    "created_at": "2026-07-21T08:00:01Z",
                    "updated_at": "2026-07-21T08:00:01Z",
                    "attachments": [],
                    "files": []
                }
            ]
        }
    ])
}

#[test]
fn test_parse_conversation_basic() {
    let convs = sample_conversations();
    let first = &convs.as_array().unwrap()[0];
    let parsed = parse_conversation_json(first, "test").unwrap();
    let session = parsed.session;

    assert_eq!(session.metadata.tool, Tool::ClaudeChat);
    assert_eq!(session.metadata.session_id, "aaa-111");
    assert_eq!(
        session.metadata.title.as_deref(),
        Some("Canned tuna storage requirements")
    );
    assert_eq!(session.metadata.project_path, "claude-chat");
    assert_eq!(session.stats.user_messages, 1);
    assert_eq!(session.stats.assistant_messages, 1);
    assert!(
        session
            .messages
            .iter()
            .any(|m| m.content.iter().any(|b| matches!(b, ContentBlock::Thinking { .. })))
    );
    assert_eq!(parsed.training.source.tool, "claude-chat");
    assert_eq!(
        parsed.training.source.source_format.as_deref(),
        Some("claude-chat-export")
    );
}

#[test]
fn test_parse_tool_use_and_result_with_null_ids() {
    let value = json!({
        "uuid": "tool-session",
        "name": "Artifacts",
        "summary": "",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:01:00Z",
        "chat_messages": [
            {
                "uuid": "a1",
                "text": "",
                "content": [
                    {
                        "type": "tool_use",
                        "id": null,
                        "name": "artifacts",
                        "input": {"command": "create", "title": "note"}
                    },
                    {
                        "type": "tool_result",
                        "tool_use_id": null,
                        "name": "artifacts",
                        "is_error": false,
                        "content": [{"type": "text", "text": "OK"}]
                    },
                    {
                        "type": "text",
                        "text": "Created."
                    }
                ],
                "sender": "assistant",
                "created_at": "2026-01-01T00:00:30Z"
            }
        ]
    });

    let session = parse_conversation_json(&value, "test").unwrap().session;
    assert_eq!(session.stats.assistant_messages, 1);
    assert_eq!(session.stats.tool_calls, 1);
    assert!(
        session.messages[0]
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { name, .. } if name == "artifacts"))
    );
    assert!(
        session.messages[0]
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { success: true, .. }))
    );
}

#[test]
fn test_discover_and_parse_from_json_file() {
    let dir = temp_path("json");
    fs::create_dir_all(&dir).unwrap();
    let json_path = dir.join("conversations.json");
    fs::write(&json_path, sample_conversations().to_string()).unwrap();

    let files = discover_export_sessions(&json_path).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().all(|(t, _)| *t == Tool::ClaudeChat));

    let parser = ClaudeChatParser;
    let parsed = parser.parse_export(&files[0].1).unwrap();
    assert_eq!(parsed.session.metadata.session_id, "aaa-111");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_discover_and_parse_from_zip() {
    let dir = temp_path("zip");
    fs::create_dir_all(&dir).unwrap();
    let zip_path = dir.join("export.zip");
    let body = sample_conversations().to_string();
    write_test_zip(&zip_path, body.as_bytes()).unwrap();

    let files = discover_export_sessions(&zip_path).unwrap();
    assert_eq!(files.len(), 2);

    let parser = ClaudeChatParser;
    let parsed = parser.parse_export(&files[1].1).unwrap();
    assert_eq!(parsed.session.metadata.session_id, "bbb-222");
    assert!(parsed.session.metadata.title.is_none());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_discover_from_directory() {
    let dir = temp_path("dir");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("conversations.json"), sample_conversations().to_string()).unwrap();

    let files = discover_export_sessions(&dir).unwrap();
    assert_eq!(files.len(), 2);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_virtual_path_roundtrip() {
    let export = PathBuf::from("/tmp/export.zip");
    let vp = virtual_path(&export, "abc-123");
    let (root, id) = split_virtual_path(&vp).unwrap();
    assert_eq!(root, export);
    assert_eq!(id, "abc-123");
    assert_eq!(export_root_from_virtual(&vp).unwrap(), export);
}
