//! Core AST types for the cassio transcript pipeline.
//!
//! # Architecture overview
//!
//! Cassio converts AI coding session logs from different tools into a unified
//! intermediate representation before formatting. This module defines that
//! representation — the AST layer.
//!
//! ```text
//! Input (JSONL/JSON) → Parser → Session (AST) → Formatter → Output (.md / .jsonl)
//! ```
//!
//! # Design philosophy
//!
//! Each tool has wildly different log schemas. Rather than letting formatting logic
//! know about every tool's quirks, parsers normalize everything into this shared AST.
//! Formatters then only need to understand the AST, not the raw log formats.
//!
//! # TRADE-OFFS
//!
//! - `ContentBlock` uses a tagged enum to allow heterogeneous message content without
//!   boxing. This means all variants must be known at compile time — adding a new
//!   content type requires touching this file and every exhaustive match on it.
//! - `SessionStats` accumulates file-operation tracking using `HashSet` rather than a
//!   count so that callers can deduplicate paths. Memory use is bounded by the number
//!   of distinct file paths in a session, which is acceptable.
//! - Token fields are `u64` rather than `Option<u64>` — parsers default to 0 when
//!   data is absent, so formatters never need to handle missing token counts.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Identifies which AI coding tool produced a session log.
///
/// WHY: Different tools write fundamentally different JSONL schemas. Carrying the
/// tool identity through the AST lets formatters and output-path derivation apply
/// tool-specific display logic (e.g., Codex calls them "Function calls" rather
/// than "Tool calls") without re-inspecting raw data.
///
/// TRADE-OFF: `ClaudeDesktop` is serialized as `"claude"` in display output because
/// end users think of it as "claude" regardless of the storage backend — keeping the
/// distinction internal-only avoids breaking output path conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Claude,
    #[serde(rename = "claude_desktop")]
    ClaudeDesktop,
    Codex,
    Hermes,
    OpenCode,
    Pi,
    Grok,
    Cursor,
}

impl std::fmt::Display for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Tool::Claude => write!(f, "claude"),
            // WHY: ClaudeDesktop is an internal distinction; users always call it "claude"
            Tool::ClaudeDesktop => write!(f, "claude"),
            Tool::Codex => write!(f, "codex"),
            Tool::Hermes => write!(f, "hermes"),
            Tool::OpenCode => write!(f, "opencode"),
            Tool::Pi => write!(f, "pi"),
            Tool::Grok => write!(f, "grok"),
            Tool::Cursor => write!(f, "cursor"),
        }
    }
}

/// A complete, normalized AI coding session ready for formatting.
///
/// WHY: Grouping metadata, messages, and stats into one struct lets formatters
/// make a single pass over all the information they need without carrying extra
/// context through function arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub metadata: SessionMetadata,
    pub messages: Vec<Message>,
    pub stats: SessionStats,
}

/// Session-level metadata extracted from the tool's log header.
///
/// WHY: Separating metadata from the message stream makes it straightforward for
/// formatters to emit a header section before iterating messages. Fields are
/// `Option` when tools differ on what they record — parsers fill in what they
/// can and leave the rest as `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub tool: Tool,
    pub project_path: String,
    pub started_at: DateTime<Utc>,
    /// Heuristic classification of whether this looks like a direct user
    /// conversation or a delegated/sub-agent execution.
    pub session_kind: SessionKind,
    /// CLI version string; only provided by Claude Code and Codex.
    pub version: Option<String>,
    /// Git branch at the time of the session, if recorded.
    pub git_branch: Option<String>,
    /// Final model used in the session; updated as model-change events are parsed.
    pub model: Option<String>,
    /// Human-readable session title, if the source records one.
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Human,
    Delegated,
    Uncertain,
}

impl std::fmt::Display for SessionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionKind::Human => write!(f, "human"),
            SessionKind::Delegated => write!(f, "delegated"),
            SessionKind::Uncertain => write!(f, "uncertain"),
        }
    }
}

/// Speaker role within a conversation turn.
///
/// WHY: Using a typed enum rather than a raw string prevents formatters from
/// accidentally matching against misspelled role names and enables exhaustive
/// pattern matching.
///
/// `System` is used for synthetic events injected by cassio (model changes,
/// queue operations) that have no equivalent speaker in the original conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

/// A single conversation turn, which may contain multiple content blocks.
///
/// WHY: Grouping all content for a turn into one `Message` preserves the
/// conversational structure. A message can contain mixed content — for example,
/// an assistant turn that includes a text response followed by a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    /// Timestamp of the message; `None` when the log format does not record it.
    pub timestamp: Option<DateTime<Utc>>,
    /// The model that produced this message; `None` for user messages.
    pub model: Option<String>,
    pub content: Vec<ContentBlock>,
    /// Per-message token usage; `None` for user messages and tool-result messages.
    pub usage: Option<TokenUsage>,
}

/// A typed unit of content within a message.
///
/// WHY: Different content types need different display treatment. Using an enum
/// rather than a map of arbitrary fields allows formatters to match exhaustively
/// and handle each case explicitly without runtime field inspection.
///
/// TRADE-OFF: `ToolUse` stores the raw `serde_json::Value` input because tool
/// inputs differ per tool name and we don't want to define typed structs for each
/// one at the AST level. Formatting is handled by the parser module's
/// `format_tool_input` helper.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text from the user or assistant.
    Text { text: String },
    /// Extended-thinking block from Claude — captured but typically hidden in output.
    Thinking { text: String },
    /// Tool invocation from the assistant; paired with a `ToolResult` in the next turn.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Outcome of a tool call, cross-referenced by `tool_use_id`.
    ///
    /// WHY: Storing a human-readable `summary` here (rather than the raw result
    /// content) avoids embedding potentially large tool outputs in the AST.
    ToolResult {
        tool_use_id: String,
        name: String,
        success: bool,
        summary: String,
    },
    /// Synthetic event recording that the active model changed during the session.
    ModelChange { model: String },
    /// Claude Code queue operation (task sub-agent handoff).
    QueueOperation { summary: String },
}

/// Anthropic-style token usage counts for a single message.
///
/// WHY: Tracking cache tokens separately from regular input tokens lets the
/// summary formatter report cache hit rates, which are significant for cost
/// optimization. Default is zero for all fields so accumulation is safe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
}

/// Aggregate statistics computed during parsing.
///
/// WHY: Pre-computing stats during the single parsing pass avoids a second
/// scan over the message list when the formatter needs summary numbers.
/// Using `HashSet<String>` for file paths deduplicates across multiple
/// tool calls to the same file within one session.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionStats {
    pub user_messages: u32,
    pub assistant_messages: u32,
    pub tool_calls: u32,
    pub tool_errors: u32,
    pub total_tokens: TokenUsage,
    pub files_read: HashSet<String>,
    pub files_written: HashSet<String>,
    pub files_edited: HashSet<String>,
    /// Wall-clock duration in seconds, computed as (last_timestamp - first_timestamp).
    /// `None` when the log contains fewer than two timestamped records.
    pub duration_seconds: Option<i64>,
    /// Total session cost in USD, if the source records one.
    pub cost: Option<f64>,
}

const DELEGATED_PROMPT_PREFIXES: &[&str] = &[
    "you are ",
    "your job is ",
    "you are a ",
    "you are an ",
];

const DELEGATED_CONTENT_MARKERS: &[&str] = &[
    "do not modify",
    "do not change files",
    "do not execute",
    "focus on ",
    "output format",
    "provide a report",
    "report only",
    "analyze as data",
    "do not modify code",
    "just analyze",
    "just review",
    "do not write code",
    "proceed to implementation",
    "limit test updates",
];

pub fn classify_session_kind(messages: &[Message]) -> SessionKind {
    let Some(first_user_text) = first_user_text(messages) else {
        return SessionKind::Uncertain;
    };

    let text = first_user_text.trim();
    let lower = text.to_lowercase();

    if DELEGATED_PROMPT_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return SessionKind::Delegated;
    }

    let marker_hits = DELEGATED_CONTENT_MARKERS
        .iter()
        .filter(|marker| lower.contains(**marker))
        .count();
    if marker_hits >= 2 {
        return SessionKind::Delegated;
    }

    if text.len() > 240
        && (lower.contains("identify where")
            || lower.contains("provide a diagram")
            || lower.contains("specifically")
            || lower.contains("pinpoint")
            || lower.contains("list "))
    {
        return SessionKind::Delegated;
    }

    SessionKind::Human
}

fn first_user_text(messages: &[Message]) -> Option<&str> {
    for message in messages {
        if message.role != Role::User {
            continue;
        }
        for block in &message.content {
            if let ContentBlock::Text { text } = block {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "ast_test.rs"]
mod tests;
