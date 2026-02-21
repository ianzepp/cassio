//! Emoji-prefixed plain-text formatter (the default output format).
//!
//! # Architecture overview
//!
//! This formatter converts the normalized `Session` AST into a human-readable
//! plain text file where every line begins with an emoji that classifies its
//! content at a glance:
//!
//! | Emoji | Meaning                          |
//! |-------|----------------------------------|
//! | 📋    | Metadata / summary               |
//! | 👤    | User message                     |
//! | 🤖    | Assistant message                |
//! | ✅    | Successful tool call             |
//! | ❌    | Failed tool call                 |
//! | ⏳    | Queue operation (sub-agent task) |
//!
//! # Design philosophy
//!
//! The output is optimized for `grep`. Each line is self-contained — the leading
//! emoji is always the first character, making patterns like `grep "❌" file.txt`
//! instant to write and read.
//!
//! The formatter is intentionally lossy in some areas:
//! - `Thinking` blocks are suppressed (they contain internal LLM reasoning, not user-visible content)
//! - `ToolUse` blocks are suppressed (they're paired with `ToolResult` which is shown)
//! - Long tool summaries are pre-truncated by the parser, not the formatter
//!
//! # TRADE-OFFS
//!
//! Using Unicode escape sequences for emoji constants (`\u{1f4cb}`) rather than
//! literal emoji in source code avoids editor encoding issues and makes the intent
//! explicit when reading the source. The compiled output is identical.

use std::io::Write;

use crate::ast::*;
use crate::error::CassioError;
use crate::formatter::Formatter;

const EMOJI_META: &str = "\u{1f4cb}";      // 📋
const EMOJI_USER: &str = "\u{1f464}";      // 👤
const EMOJI_ASSISTANT: &str = "\u{1f916}"; // 🤖
const EMOJI_SUCCESS: &str = "\u{2705}";    // ✅
const EMOJI_FAILURE: &str = "\u{274c}";    // ❌
const EMOJI_QUEUE: &str = "\u{23f3}";      // ⏳

/// Formatter that produces emoji-prefixed plain text transcripts.
pub struct EmojiTextFormatter;

impl Formatter for EmojiTextFormatter {
    /// Format a session as emoji-prefixed plain text.
    ///
    /// Output structure:
    /// 1. Metadata header (session ID, project, start time, version, branch)
    /// 2. Blank line
    /// 3. All messages in chronological order
    /// 4. Summary block (only when the session has at least one message)
    fn format(&self, session: &Session, writer: &mut dyn Write) -> Result<(), CassioError> {
        format_metadata(&session.metadata, writer)?;
        writeln!(writer)?;

        for msg in &session.messages {
            format_message(msg, writer)?;
        }

        format_summary(&session.stats, &session.metadata, writer)?;

        Ok(())
    }
}

/// Emit the session header block.
///
/// Tool-specific fields (version for Claude, CLI version for Codex, title for
/// OpenCode) are included only when present to avoid empty lines in the output.
fn format_metadata(meta: &SessionMetadata, w: &mut dyn Write) -> Result<(), CassioError> {
    writeln!(w, "{EMOJI_META} Session: {}", meta.session_id)?;
    writeln!(w, "{EMOJI_META} Project: {}", meta.project_path)?;
    writeln!(
        w,
        "{EMOJI_META} Started: {}",
        meta.started_at.to_rfc3339()
    )?;

    match meta.tool {
        Tool::Claude | Tool::ClaudeDesktop => {
            if let Some(ref version) = meta.version {
                writeln!(w, "{EMOJI_META} Version: {version}")?;
            }
        }
        Tool::Codex => {
            if let Some(ref version) = meta.version {
                writeln!(w, "{EMOJI_META} CLI: codex {version}")?;
            }
        }
        Tool::OpenCode => {
            if let Some(ref title) = meta.title {
                writeln!(w, "{EMOJI_META} Title: {title}")?;
            }
        }
    }

    if let Some(ref branch) = meta.git_branch {
        writeln!(w, "{EMOJI_META} Branch: {branch}")?;
    }

    Ok(())
}

/// Emit all content blocks within a single message.
///
/// Role determines the emoji for text content. Thinking and ToolUse blocks are
/// silently suppressed — thinking is internal LLM reasoning not intended for
/// transcripts, and ToolUse is paired with the ToolResult which carries the
/// visible output.
fn format_message(msg: &Message, w: &mut dyn Write) -> Result<(), CassioError> {
    for block in &msg.content {
        match block {
            ContentBlock::Text { text } => {
                let emoji = match msg.role {
                    Role::User => EMOJI_USER,
                    Role::Assistant => EMOJI_ASSISTANT,
                    Role::System => EMOJI_META,
                };
                writeln!(w, "{emoji} {text}")?;
            }
            ContentBlock::Thinking { .. } => {
                // WHY: Thinking blocks contain extended reasoning tokens. They are
                // not part of the conversation visible to the user and add noise.
            }
            ContentBlock::ToolUse { .. } => {
                // WHY: Tool use is deferred — the ToolResult that follows contains
                // both the tool name and the outcome, which is more informative.
            }
            ContentBlock::ToolResult {
                name,
                success,
                summary,
                ..
            } => {
                let emoji = if *success { EMOJI_SUCCESS } else { EMOJI_FAILURE };
                writeln!(w, "{emoji} {name}: {summary}")?;
            }
            ContentBlock::ModelChange { model } => {
                let short = shorten_model_name(model);
                writeln!(w, "{EMOJI_META} Model: {short}")?;
            }
            ContentBlock::QueueOperation { summary } => {
                writeln!(w, "{EMOJI_QUEUE} {summary}")?;
            }
        }
    }
    Ok(())
}

/// Emit the session summary block at the end of the transcript.
///
/// Omitted entirely when the session has no user or assistant messages — this
/// avoids a confusing empty summary for sessions that only contain tool calls
/// or were parsed from empty/truncated log files.
///
/// The "Tool calls" label is adjusted to "Function calls" for Codex sessions
/// to match the terminology used in that tool's own UI.
fn format_summary(
    stats: &SessionStats,
    metadata: &SessionMetadata,
    w: &mut dyn Write,
) -> Result<(), CassioError> {
    if stats.user_messages == 0 && stats.assistant_messages == 0 {
        return Ok(());
    }

    writeln!(w)?;
    writeln!(w, "{EMOJI_META} --- Summary ---")?;

    // Duration
    if let Some(secs) = stats.duration_seconds {
        let duration = format_duration(secs);
        writeln!(w, "{EMOJI_META} Duration: {duration}")?;
    }

    // Model (for Codex where it's tracked differently)
    if metadata.tool == Tool::Codex {
        if let Some(ref model) = metadata.model {
            writeln!(w, "{EMOJI_META} Model: {model}")?;
        }
    }

    writeln!(
        w,
        "{EMOJI_META} Messages: {} user, {} assistant",
        stats.user_messages, stats.assistant_messages
    )?;

    // Tool calls label varies by tool
    let tool_label = match metadata.tool {
        Tool::Codex => "Function calls",
        _ => "Tool calls",
    };
    writeln!(
        w,
        "{EMOJI_META} {tool_label}: {} total, {} failed",
        stats.tool_calls, stats.tool_errors
    )?;

    // Files
    let files_read = stats.files_read.len();
    let files_written = stats.files_written.len();
    let files_edited = stats.files_edited.len();
    if files_read > 0 || files_written > 0 || files_edited > 0 {
        let mut parts = Vec::new();
        if files_read > 0 {
            parts.push(format!("{files_read} read"));
        }
        if files_written > 0 {
            parts.push(format!("{files_written} written"));
        }
        if files_edited > 0 {
            parts.push(format!("{files_edited} edited"));
        }
        writeln!(w, "{EMOJI_META} Files: {}", parts.join(", "))?;
    }

    // Tokens
    let input_tokens = stats.total_tokens.input_tokens;
    let output_tokens = stats.total_tokens.output_tokens;
    if input_tokens > 0 || output_tokens > 0 {
        writeln!(
            w,
            "{EMOJI_META} Tokens: {} in, {} out",
            format_tokens(input_tokens),
            format_tokens(output_tokens)
        )?;
    }

    // Cache
    let cache_read = stats.total_tokens.cache_read_tokens;
    let cache_creation = stats.total_tokens.cache_creation_tokens;
    if cache_read > 0 || cache_creation > 0 {
        writeln!(
            w,
            "{EMOJI_META} Cache: {} read, {} created",
            format_tokens(cache_read),
            format_tokens(cache_creation)
        )?;
    }

    // Cost (OpenCode)
    if let Some(cost) = stats.cost {
        if cost > 0.0 {
            writeln!(w, "{EMOJI_META} Cost: ${cost:.4}")?;
        }
    }

    Ok(())
}

/// Abbreviate a Claude model identifier to a compact, readable name.
///
/// Transforms `claude-opus-4-5-20251101` → `opus-4.5`. Non-Claude model names
/// (e.g., OpenAI models like `gpt-4o`) and the special `"<synthetic>"` marker
/// are returned unchanged or mapped to `"synthetic"` respectively.
///
/// WHY: Full Claude model identifiers include trailing date stamps (e.g.,
/// `20251101`) that are not useful to readers and make the line visually noisy.
fn shorten_model_name(model: &str) -> String {
    if model == "<synthetic>" {
        return "synthetic".to_string();
    }

    // Pattern: claude-{name}-{major}-{minor}-{date}
    // Target:  {name}-{major}.{minor}
    let parts: Vec<&str> = model.split('-').collect();
    if parts.len() >= 4 && parts[0] == "claude" {
        if let (Ok(major), Ok(minor)) = (
            parts[2].parse::<u32>(),
            parts[3].parse::<u32>(),
        ) {
            return format!("{}-{major}.{minor}", parts[1]);
        }
    }
    model.to_string()
}

/// Format a duration in seconds as a human-readable string.
///
/// Uses the largest non-zero unit: hours+minutes, minutes only, or seconds.
/// Negative durations (clock skew) are clamped to zero.
fn format_duration(seconds: i64) -> String {
    if seconds < 0 {
        return "0s".to_string();
    }

    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{secs}s")
    }
}

/// Format a token count with SI-style suffixes (K, M).
///
/// Keeps the output compact — `1500` becomes `1.5K` rather than `1,500`.
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use chrono::Utc;

    #[test]
    fn test_shorten_model_name_opus() {
        assert_eq!(shorten_model_name("claude-opus-4-5-20251101"), "opus-4.5");
    }

    #[test]
    fn test_shorten_model_name_sonnet() {
        assert_eq!(shorten_model_name("claude-sonnet-4-5-20250929"), "sonnet-4.5");
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
                    content: vec![ContentBlock::Text { text: "Hello!".to_string() }],
                    usage: None,
                },
                Message {
                    role: Role::Assistant,
                    timestamp: Some("2025-01-15T10:00:02Z".parse().unwrap()),
                    model: Some("claude-sonnet-4-5-20250929".to_string()),
                    content: vec![ContentBlock::Text { text: "Hi there!".to_string() }],
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
        let session = make_test_session();
        let mut buf = Vec::new();
        EmojiTextFormatter.format(&session, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();

        assert!(output.contains("Session: test-session"));
        assert!(output.contains("Project: /home/user/project"));
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
        let session = Session {
            metadata: SessionMetadata {
                session_id: "s1".to_string(),
                tool: Tool::Claude,
                project_path: "/proj".to_string(),
                started_at: Utc::now(),
                version: None, git_branch: None, model: None, title: None,
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
            stats: SessionStats { user_messages: 0, assistant_messages: 1, ..Default::default() },
        };
        let mut buf = Vec::new();
        EmojiTextFormatter.format(&session, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("✅ Read: file=\"test.rs\""));
    }

    #[test]
    fn test_format_tool_result_failure() {
        let session = Session {
            metadata: SessionMetadata {
                session_id: "s1".to_string(),
                tool: Tool::Claude,
                project_path: "/proj".to_string(),
                started_at: Utc::now(),
                version: None, git_branch: None, model: None, title: None,
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
            stats: SessionStats { user_messages: 0, assistant_messages: 1, ..Default::default() },
        };
        let mut buf = Vec::new();
        EmojiTextFormatter.format(&session, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("❌ Bash: exit code 1"));
    }

    #[test]
    fn test_format_empty_stats_no_summary() {
        let session = Session {
            metadata: SessionMetadata {
                session_id: "s1".to_string(),
                tool: Tool::Claude,
                project_path: "/proj".to_string(),
                started_at: Utc::now(),
                version: None, git_branch: None, model: None, title: None,
            },
            messages: vec![],
            stats: SessionStats::default(),
        };
        let mut buf = Vec::new();
        EmojiTextFormatter.format(&session, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(!output.contains("Summary"));
    }
}
