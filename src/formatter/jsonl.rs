use std::io::Write;

use crate::error::CassioError;
use crate::formatter::Formatter;
use crate::training::ParsedSession;

pub struct JsonlFormatter;

impl Formatter for JsonlFormatter {
    fn format(&self, parsed: &ParsedSession, writer: &mut dyn Write) -> Result<(), CassioError> {
        let session = &parsed.session;
        // Output each message as a JSONL line, with metadata on the first line
        let meta_line = serde_json::to_string(&session.metadata)?;
        writeln!(writer, "{meta_line}")?;

        for msg in &session.messages {
            let line = serde_json::to_string(msg)?;
            writeln!(writer, "{line}")?;
        }

        // Stats as final line
        let stats_line = serde_json::to_string(&session.stats)?;
        writeln!(writer, "{stats_line}")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;
    use crate::training::{
        TrainingMetadata, TrainingSession, TrainingSource, training_stats_from_session,
    };
    use chrono::Utc;

    #[test]
    fn test_jsonl_output_valid_json_lines() {
        let session = Session {
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
                role: Role::User,
                timestamp: None,
                model: None,
                content: vec![ContentBlock::Text {
                    text: "hello".to_string(),
                }],
                usage: None,
            }],
            stats: SessionStats::default(),
        };
        let parsed = ParsedSession {
            training: TrainingSession::new(
                "test.v1",
                TrainingSource {
                    tool: "claude".to_string(),
                    source_path: "/tmp/source.jsonl".to_string(),
                    session_id: "s1".to_string(),
                    source_hash: "sha256:test".to_string(),
                    source_record_count: Some(1),
                    source_format: Some("jsonl".to_string()),
                    source_root: None,
                },
                TrainingMetadata {
                    project_path_raw: "/proj".to_string(),
                    project_path_sanitized: "/proj".to_string(),
                    started_at: Utc::now(),
                    ended_at: None,
                    git_branch: None,
                    title: None,
                    session_kind: "human".to_string(),
                    models_seen: Vec::new(),
                    version: None,
                },
                training_stats_from_session(&SessionStats::default()),
            ),
            session,
        };

        let mut buf = Vec::new();
        JsonlFormatter.format(&parsed, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();

        let lines: Vec<&str> = output.trim().lines().collect();
        assert_eq!(lines.len(), 3); // metadata + 1 message + stats

        // Each line should be valid JSON
        for line in &lines {
            assert!(
                serde_json::from_str::<serde_json::Value>(line).is_ok(),
                "Invalid JSON: {line}"
            );
        }

        // First line should contain session_id
        assert!(lines[0].contains("s1"));
    }
}
