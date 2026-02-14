use std::io::Write;

use crate::ast::*;
use crate::error::CassioError;
use crate::formatter::Formatter;

const EMOJI_META: &str = "\u{1f4cb}";     // 📋
const EMOJI_USER: &str = "\u{1f464}";     // 👤
const EMOJI_ASSISTANT: &str = "\u{1f916}"; // 🤖
const EMOJI_SUCCESS: &str = "\u{2705}";   // ✅
const EMOJI_FAILURE: &str = "\u{274c}";   // ❌
const EMOJI_QUEUE: &str = "\u{23f3}";     // ⏳

pub struct EmojiTextFormatter;

impl Formatter for EmojiTextFormatter {
    fn format(&self, session: &Session, writer: &mut dyn Write) -> Result<(), CassioError> {
        // Metadata header
        format_metadata(&session.metadata, writer)?;
        writeln!(writer)?;

        // Messages
        for msg in &session.messages {
            format_message(msg, writer)?;
        }

        // Summary
        format_summary(&session.stats, &session.metadata, writer)?;

        Ok(())
    }
}

fn format_metadata(meta: &SessionMetadata, w: &mut dyn Write) -> Result<(), CassioError> {
    writeln!(w, "{EMOJI_META} Session: {}", meta.session_id)?;
    writeln!(w, "{EMOJI_META} Project: {}", meta.project_path)?;
    writeln!(
        w,
        "{EMOJI_META} Started: {}",
        meta.started_at.to_rfc3339()
    )?;

    match meta.tool {
        Tool::Claude => {
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
                // Skip thinking blocks in emoji-text output
            }
            ContentBlock::ToolUse { .. } => {
                // Tool use is shown when we get the result
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

fn shorten_model_name(model: &str) -> String {
    if model == "<synthetic>" {
        return "synthetic".to_string();
    }

    // claude-opus-4-5-20251101 -> opus-4.5
    // claude-sonnet-4-5-20250929 -> sonnet-4.5
    let parts: Vec<&str> = model.split('-').collect();
    if parts.len() >= 4 && parts[0] == "claude" {
        // Try to find name-major-minor pattern
        if let (Ok(major), Ok(minor)) = (
            parts[2].parse::<u32>(),
            parts[3].parse::<u32>(),
        ) {
            return format!("{}-{major}.{minor}", parts[1]);
        }
    }
    model.to_string()
}

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

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
