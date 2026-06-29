use super::*;
use crate::training::{
    ParsedSession, TrainingMetadata, TrainingSession, TrainingSource, training_stats_from_session,
};
use chrono::Utc;
use std::collections::HashSet;

fn parsed_from_session(session: Session) -> ParsedSession {
    ParsedSession {
        training: TrainingSession::new(
            "test.v1",
            TrainingSource {
                tool: session.metadata.tool.to_string(),
                source_path: "/tmp/source".to_string(),
                session_id: session.metadata.session_id.clone(),
                source_hash: "sha256:test".to_string(),
                source_record_count: Some(1),
                source_format: Some("jsonl".to_string()),
                source_root: None,
            },
            TrainingMetadata {
                project_path_raw: session.metadata.project_path.clone(),
                project_path_sanitized: session.metadata.project_path.clone(),
                started_at: session.metadata.started_at,
                ended_at: None,
                git_branch: session.metadata.git_branch.clone(),
                title: session.metadata.title.clone(),
                session_kind: session.metadata.session_kind.to_string(),
                models_seen: session.metadata.model.clone().into_iter().collect(),
                version: session.metadata.version.clone(),
            },
            training_stats_from_session(&session.stats),
        ),
        session,
    }
}

#[test]
fn test_shorten_model_name_opus() {
    assert_eq!(shorten_model_name("claude-opus-4-5-20251101"), "opus-4.5");
}

#[test]
fn test_shorten_model_name_sonnet() {
    assert_eq!(
        shorten_model_name("claude-sonnet-4-5-20250929"),
        "sonnet-4.5"
    );
}

#[test]
fn test_shorten_model_name_synthetic() {
    assert_eq!(shorten_model_name("<synthetic>"), "synthetic");
}

#[test]
fn test_shorten_model_name_unknown() {
    assert_eq!(shorten_model_name("gpt-4o"), "gpt-4o");
}

#[test]
fn test_format_duration_seconds() {
    assert_eq!(format_duration(45), "45s");
}

#[test]
fn test_format_duration_minutes() {
    assert_eq!(format_duration(300), "5m");
}

#[test]
fn test_format_duration_hours() {
    assert_eq!(format_duration(5400), "1h 30m");
}

#[test]
fn test_format_duration_zero() {
    assert_eq!(format_duration(0), "0s");
}

#[test]
fn test_format_duration_negative() {
    assert_eq!(format_duration(-5), "0s");
}

#[test]
fn test_format_tokens_small() {
    assert_eq!(format_tokens(500), "500");
}

#[test]
fn test_format_tokens_thousands() {
    assert_eq!(format_tokens(1500), "1.5K");
}

#[test]
fn test_format_tokens_millions() {
    assert_eq!(format_tokens(1_500_000), "1.5M");
}

#[test]
fn test_format_tokens_zero() {
    assert_eq!(format_tokens(0), "0");
}

fn make_test_session() -> Session {
    Session {
        metadata: SessionMetadata {
            session_id: "test-session".to_string(),
            tool: Tool::Claude,
            project_path: "/home/user/project".to_string(),
            started_at: "2025-01-15T10:00:00Z".parse().unwrap(),
            session_kind: SessionKind::Human,
            version: Some("1.0.0".to_string()),
            git_branch: Some("main".to_string()),
            model: Some("claude-sonnet-4-5-20250929".to_string()),
            title: None,
        },
        messages: vec![
            Message {
                role: Role::User,
                timestamp: Some("2025-01-15T10:00:01Z".parse().unwrap()),
                model: None,
                content: vec![ContentBlock::Text {
                    text: "Hello!".to_string(),
                }],
                usage: None,
            },
            Message {
                role: Role::Assistant,
                timestamp: Some("2025-01-15T10:00:02Z".parse().unwrap()),
                model: Some("claude-sonnet-4-5-20250929".to_string()),
                content: vec![ContentBlock::Text {
                    text: "Hi there!".to_string(),
                }],
                usage: None,
            },
        ],
        stats: SessionStats {
            user_messages: 1,
            assistant_messages: 1,
            tool_calls: 2,
            tool_errors: 0,
            total_tokens: TokenUsage {
                input_tokens: 1500,
                output_tokens: 500,
                cache_read_tokens: 100,
                cache_creation_tokens: 50,
            },
            files_read: HashSet::from(["foo.rs".to_string()]),
            files_written: HashSet::new(),
            files_edited: HashSet::new(),
            duration_seconds: Some(120),
            cost: None,
        },
    }
}

#[test]
fn test_full_format_output() {
    let session = parsed_from_session(make_test_session());
    let mut buf = Vec::new();
    EmojiTextFormatter.format(&session, &mut buf).unwrap();
    let output = String::from_utf8(buf).unwrap();

    assert!(output.contains("Session: test-session"));
    assert!(output.contains("Project: /home/user/project"));
    assert!(output.contains("Session Kind: human"));
    assert!(output.contains("Version: 1.0.0"));
    assert!(output.contains("Branch: main"));
    assert!(output.contains("👤 Hello!"));
    assert!(output.contains("🤖 Hi there!"));
    assert!(output.contains("--- Summary ---"));
    assert!(output.contains("Duration: 2m"));
    assert!(output.contains("Messages: 1 user, 1 assistant"));
    assert!(output.contains("Tool calls: 2 total, 0 failed"));
    assert!(output.contains("Tokens:"));
    assert!(output.contains("Files: 1 read"));
}

#[test]
fn test_format_tool_result_success() {
    let session = parsed_from_session(Session {
        metadata: SessionMetadata {
            session_id: "s1".to_string(),
            tool: Tool::Claude,
            project_path: "/proj".to_string(),
            started_at: Utc::now(),
            session_kind: SessionKind::Human,
            version: None,
            git_branch: None,
            model: None,
            title: None,
        },
        messages: vec![Message {
            role: Role::Assistant,
            timestamp: None,
            model: None,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                name: "Read".to_string(),
                success: true,
                summary: "file=\"test.rs\"".to_string(),
            }],
            usage: None,
        }],
        stats: SessionStats {
            user_messages: 0,
            assistant_messages: 1,
            ..Default::default()
        },
    });
    let mut buf = Vec::new();
    EmojiTextFormatter.format(&session, &mut buf).unwrap();
    let output = String::from_utf8(buf).unwrap();
    assert!(output.contains("✅ Read: file=\"test.rs\""));
}

#[test]
fn test_format_tool_result_failure() {
    let session = parsed_from_session(Session {
        metadata: SessionMetadata {
            session_id: "s1".to_string(),
            tool: Tool::Claude,
            project_path: "/proj".to_string(),
            started_at: Utc::now(),
            session_kind: SessionKind::Human,
            version: None,
            git_branch: None,
            model: None,
            title: None,
        },
        messages: vec![Message {
            role: Role::Assistant,
            timestamp: None,
            model: None,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                name: "Bash".to_string(),
                success: false,
                summary: "exit code 1".to_string(),
            }],
            usage: None,
        }],
        stats: SessionStats {
            user_messages: 0,
            assistant_messages: 1,
            ..Default::default()
        },
    });
    let mut buf = Vec::new();
    EmojiTextFormatter.format(&session, &mut buf).unwrap();
    let output = String::from_utf8(buf).unwrap();
    assert!(output.contains("❌ Bash: exit code 1"));
}

#[test]
fn test_format_empty_stats_no_summary() {
    let session = parsed_from_session(Session {
        metadata: SessionMetadata {
            session_id: "s1".to_string(),
            tool: Tool::Claude,
            project_path: "/proj".to_string(),
            started_at: Utc::now(),
            session_kind: SessionKind::Human,
            version: None,
            git_branch: None,
            model: None,
            title: None,
        },
        messages: vec![],
        stats: SessionStats::default(),
    });
    let mut buf = Vec::new();
    EmojiTextFormatter.format(&session, &mut buf).unwrap();
    let output = String::from_utf8(buf).unwrap();
    assert!(!output.contains("Summary"));
}
