use std::sync::OnceLock;

use regex::{Captures, Regex};

use crate::ast::{ContentBlock, Message, Session, SessionMetadata};

pub fn redact_session(session: &Session) -> Session {
    Session {
        metadata: redact_metadata(&session.metadata),
        messages: session.messages.iter().map(redact_message).collect(),
        stats: session.stats.clone(),
    }
}

pub fn redact_text(input: &str) -> String {
    let mut output = input.to_string();

    output = private_key_regex()
        .replace_all(&output, "[REDACTED PRIVATE KEY]")
        .into_owned();

    output = env_assignment_regex()
        .replace_all(&output, |caps: &Captures<'_>| {
            format!("{}{}[REDACTED]", &caps["name"], &caps["sep"])
        })
        .into_owned();

    output = bearer_regex()
        .replace_all(&output, |caps: &Captures<'_>| {
            format!("{}[REDACTED]", &caps["prefix"])
        })
        .into_owned();

    for (regex, replacement) in token_patterns() {
        output = regex.replace_all(&output, *replacement).into_owned();
    }

    output
}

fn redact_metadata(meta: &SessionMetadata) -> SessionMetadata {
    SessionMetadata {
        session_id: redact_text(&meta.session_id),
        tool: meta.tool,
        project_path: redact_text(&meta.project_path),
        started_at: meta.started_at,
        session_kind: meta.session_kind,
        version: meta.version.as_deref().map(redact_text),
        git_branch: meta.git_branch.as_deref().map(redact_text),
        model: meta.model.as_deref().map(redact_text),
        title: meta.title.as_deref().map(redact_text),
    }
}

fn redact_message(message: &Message) -> Message {
    Message {
        role: message.role,
        timestamp: message.timestamp,
        model: message.model.as_deref().map(redact_text),
        content: message.content.iter().map(redact_block).collect(),
        usage: message.usage.clone(),
    }
}

fn redact_block(block: &ContentBlock) -> ContentBlock {
    match block {
        ContentBlock::Text { text } => ContentBlock::Text {
            text: redact_text(text),
        },
        ContentBlock::Thinking { text } => ContentBlock::Thinking {
            text: redact_text(text),
        },
        ContentBlock::ToolUse { id, name, input } => ContentBlock::ToolUse {
            id: redact_text(id),
            name: redact_text(name),
            input: input.clone(),
        },
        ContentBlock::ToolResult {
            tool_use_id,
            name,
            success,
            summary,
        } => ContentBlock::ToolResult {
            tool_use_id: redact_text(tool_use_id),
            name: redact_text(name),
            success: *success,
            summary: redact_text(summary),
        },
        ContentBlock::ModelChange { model } => ContentBlock::ModelChange {
            model: redact_text(model),
        },
        ContentBlock::QueueOperation { summary } => ContentBlock::QueueOperation {
            summary: redact_text(summary),
        },
    }
}

fn private_key_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?s)-----BEGIN(?: [A-Z0-9]+)* PRIVATE KEY-----.*?-----END(?: [A-Z0-9]+)* PRIVATE KEY-----",
        )
        .unwrap()
    })
}

fn env_assignment_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(?P<name>anthropic_api_key|openai_api_key|claude_code_oauth_token|github_token|gh_token|openrouter_api_key|pinecone_api_key|langchain_api_key|aws_secret_access_key|database_url|jwt_secret_key|access_token_salt|head_api_key)(?P<sep>\s*[:=]\s*)(?:"[^"\n]*"|'[^'\n]*'|[^\s,\n]+)"#,
        )
        .unwrap()
    })
}

fn bearer_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r"(?i)(?P<prefix>\bauthorization\s*:\s*bearer\s+|\bbearer\s+)([A-Za-z0-9._-]{20,})",
        )
        .unwrap()
    })
}

fn token_patterns() -> &'static Vec<(Regex, &'static str)> {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            (
                Regex::new(r"\bsk-ant-[A-Za-z0-9_-]{20,}\b").unwrap(),
                "[REDACTED ANTHROPIC TOKEN]",
            ),
            (
                Regex::new(r"\bsk-proj-[A-Za-z0-9_-]{20,}\b").unwrap(),
                "[REDACTED OPENAI TOKEN]",
            ),
            (
                Regex::new(r"\bsk-(?:live|test)-[A-Za-z0-9]{16,}\b").unwrap(),
                "[REDACTED API TOKEN]",
            ),
            (
                Regex::new(r"\bgh[pousr]_[A-Za-z0-9]{36}\b").unwrap(),
                "[REDACTED GITHUB TOKEN]",
            ),
            (
                Regex::new(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b").unwrap(),
                "[REDACTED GITHUB TOKEN]",
            ),
            (
                Regex::new(r"\bAIza[0-9A-Za-z_-]{35}\b").unwrap(),
                "[REDACTED GOOGLE API KEY]",
            ),
            (
                Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").unwrap(),
                "[REDACTED AWS ACCESS KEY]",
            ),
            (
                Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b").unwrap(),
                "[REDACTED SLACK TOKEN]",
            ),
        ]
    })
}

#[cfg(test)]
mod tests {
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
}
