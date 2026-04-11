use std::sync::OnceLock;

use regex::{Captures, Regex};
use serde_json::Value;

use crate::ast::{ContentBlock, Message, Session, SessionMetadata};
use crate::training::ParsedSession;

#[derive(Default)]
struct RedactionAudit {
    count: u32,
    kinds: Vec<String>,
}

impl RedactionAudit {
    fn record(&mut self, kind: &str) {
        self.count += 1;
        if !self.kinds.iter().any(|existing| existing == kind) {
            self.kinds.push(kind.to_string());
        }
    }
}

pub fn redact_export(parsed: &ParsedSession) -> ParsedSession {
    let mut audit = RedactionAudit::default();
    let session = redact_session_with_audit(&parsed.session, &mut audit);
    let mut training = parsed.training.clone();

    training.metadata.project_path_sanitized =
        redact_text_with_audit(&training.metadata.project_path_raw, &mut audit);
    training.metadata.git_branch = training
        .metadata
        .git_branch
        .as_deref()
        .map(|branch| redact_text_with_audit(branch, &mut audit));
    training.metadata.title = training
        .metadata
        .title
        .as_deref()
        .map(|title| redact_text_with_audit(title, &mut audit));
    training.metadata.version = training
        .metadata
        .version
        .as_deref()
        .map(|version| redact_text_with_audit(version, &mut audit));
    training.metadata.models_seen = training
        .metadata
        .models_seen
        .iter()
        .map(|model| redact_text_with_audit(model, &mut audit))
        .collect();
    training.source.session_id = redact_text_with_audit(&training.source.session_id, &mut audit);
    training.source.source_path = redact_text_with_audit(&training.source.source_path, &mut audit);
    training.source.source_root = training
        .source
        .source_root
        .as_deref()
        .map(|root| redact_text_with_audit(root, &mut audit));

    for event in &mut training.events {
        event.role = event
            .role
            .as_deref()
            .map(|role| redact_text_with_audit(role, &mut audit));
        event.model = event
            .model
            .as_deref()
            .map(|model| redact_text_with_audit(model, &mut audit));
        event.sanitized_text = event
            .raw_text
            .as_deref()
            .map(|text| redact_text_with_audit(text, &mut audit));
        event.tool_name = event
            .tool_name
            .as_deref()
            .map(|name| redact_text_with_audit(name, &mut audit));
        event.tool_call_id = event
            .tool_call_id
            .as_deref()
            .map(|id| redact_text_with_audit(id, &mut audit));
        event.tool_input_sanitized = event
            .tool_input_raw
            .as_ref()
            .map(|value| redact_json_value(value, &mut audit));
        event.tool_output_sanitized = event
            .tool_output_raw
            .as_ref()
            .map(|value| redact_json_value(value, &mut audit));
        event.source_record_refs = event
            .source_record_refs
            .iter()
            .map(|value| redact_text_with_audit(value, &mut audit))
            .collect();
    }

    training.sanitization.redaction_count += audit.count;
    for kind in audit.kinds {
        if !training
            .sanitization
            .redaction_kinds
            .iter()
            .any(|existing| existing == &kind)
        {
            training.sanitization.redaction_kinds.push(kind);
        }
    }

    ParsedSession { session, training }
}

pub fn redact_session(session: &Session) -> Session {
    let mut audit = RedactionAudit::default();
    redact_session_with_audit(session, &mut audit)
}

fn redact_session_with_audit(session: &Session, audit: &mut RedactionAudit) -> Session {
    Session {
        metadata: redact_metadata(&session.metadata, audit),
        messages: session
            .messages
            .iter()
            .map(|message| redact_message(message, audit))
            .collect(),
        stats: session.stats.clone(),
    }
}

pub fn redact_text(input: &str) -> String {
    let mut audit = RedactionAudit::default();
    redact_text_with_audit(input, &mut audit)
}

fn redact_text_with_audit(input: &str, audit: &mut RedactionAudit) -> String {
    let mut output = input.to_string();

    let private_redacted = private_key_regex()
        .replace_all(&output, |_: &Captures<'_>| {
            audit.record("private_key");
            "[REDACTED PRIVATE KEY]"
        })
        .into_owned();
    output = private_redacted;

    output = env_assignment_regex()
        .replace_all(&output, |caps: &Captures<'_>| {
            audit.record("env_assignment");
            format!("{}{}[REDACTED]", &caps["name"], &caps["sep"])
        })
        .into_owned();

    output = bearer_regex()
        .replace_all(&output, |caps: &Captures<'_>| {
            audit.record("bearer_token");
            format!("{}[REDACTED]", &caps["prefix"])
        })
        .into_owned();

    for (regex, replacement, kind) in token_patterns() {
        output = regex
            .replace_all(&output, |_: &Captures<'_>| {
                audit.record(kind);
                *replacement
            })
            .into_owned();
    }

    output
}

fn redact_json_value(value: &Value, audit: &mut RedactionAudit) -> Value {
    match value {
        Value::String(text) => Value::String(redact_text_with_audit(text, audit)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_json_value(item, audit))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        if key.to_lowercase().contains("path") {
                            match value {
                                Value::String(text) => {
                                    Value::String(redact_text_with_audit(text, audit))
                                }
                                _ => redact_json_value(value, audit),
                            }
                        } else {
                            redact_json_value(value, audit)
                        },
                    )
                })
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn redact_metadata(meta: &SessionMetadata, audit: &mut RedactionAudit) -> SessionMetadata {
    SessionMetadata {
        session_id: redact_text_with_audit(&meta.session_id, audit),
        tool: meta.tool,
        project_path: redact_text_with_audit(&meta.project_path, audit),
        started_at: meta.started_at,
        session_kind: meta.session_kind,
        version: meta
            .version
            .as_deref()
            .map(|value| redact_text_with_audit(value, audit)),
        git_branch: meta
            .git_branch
            .as_deref()
            .map(|value| redact_text_with_audit(value, audit)),
        model: meta
            .model
            .as_deref()
            .map(|value| redact_text_with_audit(value, audit)),
        title: meta
            .title
            .as_deref()
            .map(|value| redact_text_with_audit(value, audit)),
    }
}

fn redact_message(message: &Message, audit: &mut RedactionAudit) -> Message {
    Message {
        role: message.role,
        timestamp: message.timestamp,
        model: message
            .model
            .as_deref()
            .map(|value| redact_text_with_audit(value, audit)),
        content: message
            .content
            .iter()
            .map(|block| redact_block(block, audit))
            .collect(),
        usage: message.usage.clone(),
    }
}

fn redact_block(block: &ContentBlock, audit: &mut RedactionAudit) -> ContentBlock {
    match block {
        ContentBlock::Text { text } => ContentBlock::Text {
            text: redact_text_with_audit(text, audit),
        },
        ContentBlock::Thinking { text } => ContentBlock::Thinking {
            text: redact_text_with_audit(text, audit),
        },
        ContentBlock::ToolUse { id, name, input } => ContentBlock::ToolUse {
            id: redact_text_with_audit(id, audit),
            name: redact_text_with_audit(name, audit),
            input: redact_json_value(input, audit),
        },
        ContentBlock::ToolResult {
            tool_use_id,
            name,
            success,
            summary,
        } => ContentBlock::ToolResult {
            tool_use_id: redact_text_with_audit(tool_use_id, audit),
            name: redact_text_with_audit(name, audit),
            success: *success,
            summary: redact_text_with_audit(summary, audit),
        },
        ContentBlock::ModelChange { model } => ContentBlock::ModelChange {
            model: redact_text_with_audit(model, audit),
        },
        ContentBlock::QueueOperation { summary } => ContentBlock::QueueOperation {
            summary: redact_text_with_audit(summary, audit),
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

fn token_patterns() -> &'static Vec<(Regex, &'static str, &'static str)> {
    static PATTERNS: OnceLock<Vec<(Regex, &'static str, &'static str)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            (
                Regex::new(r"\bsk-ant-[A-Za-z0-9_-]{20,}\b").unwrap(),
                "[REDACTED ANTHROPIC TOKEN]",
                "api_key",
            ),
            (
                Regex::new(r"\bsk-proj-[A-Za-z0-9_-]{20,}\b").unwrap(),
                "[REDACTED OPENAI TOKEN]",
                "api_key",
            ),
            (
                Regex::new(r"\bsk-(?:live|test)-[A-Za-z0-9]{16,}\b").unwrap(),
                "[REDACTED API TOKEN]",
                "api_key",
            ),
            (
                Regex::new(r"\bgh[pousr]_[A-Za-z0-9]{36}\b").unwrap(),
                "[REDACTED GITHUB TOKEN]",
                "api_key",
            ),
            (
                Regex::new(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b").unwrap(),
                "[REDACTED GITHUB TOKEN]",
                "api_key",
            ),
            (
                Regex::new(r"\bAIza[0-9A-Za-z_-]{35}\b").unwrap(),
                "[REDACTED GOOGLE API KEY]",
                "api_key",
            ),
            (
                Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").unwrap(),
                "[REDACTED AWS ACCESS KEY]",
                "api_key",
            ),
            (
                Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b").unwrap(),
                "[REDACTED SLACK TOKEN]",
                "api_key",
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
