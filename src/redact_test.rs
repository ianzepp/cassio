use chrono::Utc;

use super::*;
use crate::ast::{Role, SessionKind, SessionStats, TokenUsage, Tool};

#[test]
fn redacts_known_tokens() {
    let input = "export ANTHROPIC_API_KEY=sk-ant-api03-secretsecretsecret";
    let output = redact_text(input);
    assert!(output.contains("ANTHROPIC_API_KEY=[REDACTED]"));
    assert!(!output.contains("sk-ant-api03-secretsecretsecret"));
}

#[test]
fn redacts_openai_project_tokens() {
    let input = "HEAD_API_KEY=sk-proj-abcdefghijklmnopqrstuvwxyz1234567890";
    let output = redact_text(input);
    assert!(output.contains("HEAD_API_KEY=[REDACTED]"));
    assert!(!output.contains("sk-proj-abcdefghijklmnopqrstuvwxyz1234567890"));
}

#[test]
fn redacts_bearer_tokens() {
    let input = "Authorization: Bearer abcdefghijklmnopqrstuvwxyz123456";
    let output = redact_text(input);
    assert_eq!(output, "Authorization: Bearer [REDACTED]");
}

#[test]
fn redacts_private_keys() {
    let input = "-----BEGIN PRIVATE KEY-----\nabc123\n-----END PRIVATE KEY-----";
    let output = redact_text(input);
    assert_eq!(output, "[REDACTED PRIVATE KEY]");
}

#[test]
fn redacts_session_content_before_output() {
    let session = Session {
        metadata: SessionMetadata {
            session_id: "s1".to_string(),
            tool: Tool::Claude,
            project_path: "/tmp/project".to_string(),
            started_at: Utc::now(),
            session_kind: SessionKind::Human,
            version: Some("1.0.0".to_string()),
            git_branch: Some("main".to_string()),
            model: Some("claude-sonnet-4-5-20250929".to_string()),
            title: None,
        },
        messages: vec![Message {
            role: Role::Assistant,
            timestamp: None,
            model: None,
            content: vec![
                ContentBlock::Text {
                    text: "Use sk-ant-api03-secretsecretsecret".to_string(),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    name: "Bash".to_string(),
                    success: true,
                    summary: "export CLAUDE_CODE_OAUTH_TOKEN=sk-ant-oat01-supersecretsecret"
                        .to_string(),
                },
            ],
            usage: Some(TokenUsage::default()),
        }],
        stats: SessionStats::default(),
    };

    let redacted = redact_session(&session);
    let rendered = match &redacted.messages[0].content[0] {
        ContentBlock::Text { text } => text.clone(),
        _ => String::new(),
    };
    let summary = match &redacted.messages[0].content[1] {
        ContentBlock::ToolResult { summary, .. } => summary.clone(),
        _ => String::new(),
    };

    assert!(!rendered.contains("sk-ant-"));
    assert!(!summary.contains("sk-ant-"));
    assert!(summary.contains("CLAUDE_CODE_OAUTH_TOKEN=[REDACTED]"));
}
